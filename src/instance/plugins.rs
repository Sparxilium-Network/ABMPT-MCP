//! Plugin jar deployment, removal and rollback.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::{BootReport, Instance, Manager, StopReport};
use crate::{jarinfo, logs, store::sanitize_jar_filename, util};

#[derive(Debug, Clone, Serialize)]
pub struct InstalledPlugin {
    pub name: String,
    pub version: Option<String>,
    pub file: String,
}

#[derive(Debug, Serialize)]
pub struct LoadCheck {
    pub loaded: bool,
    pub evidence: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DeployReport {
    pub plugin: String,
    pub version: Option<String>,
    pub file: String,
    pub replaced: Vec<String>,
    pub backups: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot: Option<BootReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_check: Option<LoadCheck>,
    pub note: String,
}

fn backup_dirname(plugin: &str) -> String {
    plugin
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' })
        .collect()
}

impl Manager {
    fn plugins_dir(&self, name: &str) -> PathBuf {
        self.dir(name).join("plugins")
    }

    fn plugin_backup_dir(&self, inst: &str, plugin: &str) -> PathBuf {
        self.paths
            .backups_dir(inst)
            .join("plugins")
            .join(backup_dirname(plugin))
    }

    pub fn installed_plugins(&self, name: &str) -> Vec<InstalledPlugin> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.plugins_dir(name)) else {
            return out;
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() || p.extension().is_none_or(|x| x != "jar") {
                continue;
            }
            let file = e.file_name().to_string_lossy().into_owned();
            match jarinfo::plugin_info(&p) {
                Ok(i) => out.push(InstalledPlugin {
                    name: i.name,
                    version: i.version,
                    file,
                }),
                Err(_) => out.push(InstalledPlugin {
                    name: format!("(unknown: {file})"),
                    version: None,
                    file,
                }),
            }
        }
        out.sort_by_key(|p| p.name.to_lowercase());
        out
    }

    fn jars_of_plugin(&self, inst: &str, plugin: &str) -> Vec<PathBuf> {
        self.installed_plugins(inst)
            .into_iter()
            .filter(|p| p.name.eq_ignore_ascii_case(plugin))
            .map(|p| self.plugins_dir(inst).join(p.file))
            .collect()
    }

    /// Move a jar into the backup area. Backup files are named `<stamp>__<original file>`.
    fn backup_jar(&self, inst: &str, plugin: &str, jar: &Path) -> Result<String> {
        let dir = self.plugin_backup_dir(inst, plugin);
        std::fs::create_dir_all(&dir)?;
        let file = jar.file_name().context("bad jar")?.to_string_lossy().into_owned();
        let dest = dir.join(format!("{}__{file}", util::stamp()));
        std::fs::rename(jar, &dest).or_else(|_| {
            std::fs::copy(jar, &dest)?;
            std::fs::remove_file(jar)
        })?;
        Ok(dest.file_name().unwrap().to_string_lossy().into_owned())
    }

    pub fn plugin_backups(&self, inst: &str, plugin: &str) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.plugin_backup_dir(inst, plugin))
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.ends_with(".jar"))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Decide whether a plugin loaded, from the boot output.
    fn check_loaded(boot_text: &str, plugin: &str) -> LoadCheck {
        let lc = plugin.to_lowercase();
        let entries = logs::entries(boot_text);
        let mut enabled = false;
        let mut evidence = Vec::new();
        for e in &entries {
            let first = e.text.lines().next().unwrap_or("").to_lowercase();
            if first.contains(&format!("[{lc}] enabling {lc}")) {
                enabled = true;
                evidence.push(logs::shorten(e, 1));
            } else if (e.level >= logs::Level::Error
                && (first.contains(&lc) || e.text.to_lowercase().contains(&format!("plugins/{lc}"))))
                || first.contains(&format!("error occurred while enabling {lc}"))
                || (first.contains("could not load '") && first.contains(&lc))
            {
                evidence.push(logs::shorten(e, 6));
            }
        }
        let failed = evidence.iter().any(|t| {
            let l = t.to_lowercase();
            l.contains("error occurred while enabling") || l.contains("could not load") || l.contains("disabling")
        });
        evidence.truncate(10);
        LoadCheck {
            loaded: enabled && !failed,
            evidence,
        }
    }

    async fn maybe_restart(
        &self,
        inst: &Arc<Instance>,
        restart: bool,
        plugin: &str,
        report_note: &mut String,
    ) -> Result<(Option<StopReport>, Option<BootReport>, Option<LoadCheck>)> {
        let running = inst.proc().is_some();
        if !restart {
            report_note.push_str(if running {
                "Instance is running: the change takes effect after a restart."
            } else {
                "Instance is stopped: the change takes effect on next start."
            });
            return Ok((None, None, None));
        }
        if !running {
            report_note.push_str("restart=true but the instance is stopped; it was left stopped. The change takes effect on next start.");
            return Ok((None, None, None));
        }
        let (s, b) = self.restart(inst).await?;
        let check = inst.proc().map(|p| Self::check_loaded(&p.boot_text(), plugin));
        report_note.push_str("Instance was restarted.");
        Ok((Some(s), Some(b), check))
    }

    pub async fn plugin_deploy(
        &self,
        inst: &Arc<Instance>,
        upload_id: &str,
        restart: bool,
    ) -> Result<DeployReport> {
        let (up, src) = self.uploads.get(upload_id)?;
        let info = jarinfo::plugin_info(&src)?;
        let filename = sanitize_jar_filename(&up.filename)?;
        let pdir = self.plugins_dir(&inst.name);
        std::fs::create_dir_all(&pdir)?;
        let target = pdir.join(&filename);
        if target.exists()
            && let Ok(other) = jarinfo::plugin_info(&target)
            && !other.name.eq_ignore_ascii_case(&info.name)
        {
            bail!(
                "plugins/{filename} already exists and belongs to a different plugin ('{}')",
                other.name
            );
        }
        let old = self.jars_of_plugin(&inst.name, &info.name);
        let mut backups = Vec::new();
        let mut replaced = Vec::new();
        for j in &old {
            replaced.push(j.file_name().unwrap().to_string_lossy().into_owned());
            backups.push(self.backup_jar(&inst.name, &info.name, j)?);
        }
        let tmp = pdir.join(format!(".{filename}.part"));
        std::fs::copy(&src, &tmp)?;
        std::fs::rename(&tmp, &target)?;

        let mut note = String::new();
        if !backups.is_empty() {
            note.push_str("Previous version kept as backup; use plugin_rollback to restore it. ");
        }
        let (stop, boot, load_check) = self.maybe_restart(inst, restart, &info.name, &mut note).await?;
        Ok(DeployReport {
            plugin: info.name,
            version: info.version,
            file: filename,
            replaced,
            backups,
            stop,
            boot,
            load_check,
            note,
        })
    }

    pub fn plugin_remove(&self, inst: &Arc<Instance>, plugin: &str) -> Result<String> {
        let jars = self.jars_of_plugin(&inst.name, plugin);
        if jars.is_empty() {
            bail!("plugin '{plugin}' is not installed in '{}'", inst.name);
        }
        let mut saved = Vec::new();
        for j in &jars {
            saved.push(self.backup_jar(&inst.name, plugin, j)?);
        }
        let running = inst.proc().is_some();
        Ok(format!(
            "removed plugin '{plugin}' ({} jar(s)); kept as backup: {}. Its config folder was left in place.{}",
            jars.len(),
            saved.join(", "),
            if running { " The instance is running: restart it to unload the plugin." } else { "" }
        ))
    }

    pub async fn plugin_rollback(
        &self,
        inst: &Arc<Instance>,
        plugin: &str,
        restart: bool,
    ) -> Result<DeployReport> {
        let backups = self.plugin_backups(&inst.name, plugin);
        let latest = backups
            .last()
            .with_context(|| format!("no backup found for plugin '{plugin}' in '{}'", inst.name))?
            .clone();
        let bdir = self.plugin_backup_dir(&inst.name, plugin);
        let src = bdir.join(&latest);
        let info = jarinfo::plugin_info(&src)?;
        let original = latest
            .split_once("__")
            .map(|(_, f)| f.to_string())
            .unwrap_or(latest.clone());
        let filename = sanitize_jar_filename(&original)?;

        // Current version becomes a backup too, so a rollback can itself be undone.
        let current = self.jars_of_plugin(&inst.name, plugin);
        let mut replaced = Vec::new();
        let mut new_backups = Vec::new();
        for j in &current {
            replaced.push(j.file_name().unwrap().to_string_lossy().into_owned());
            new_backups.push(self.backup_jar(&inst.name, plugin, j)?);
        }
        let target = self.plugins_dir(&inst.name).join(&filename);
        std::fs::rename(&src, &target).or_else(|_| {
            std::fs::copy(&src, &target)?;
            std::fs::remove_file(&src)
        })?;
        let mut note = format!("Restored backup {latest}. ");
        if !new_backups.is_empty() {
            note.push_str("The replaced version is now the newest backup (rolling back again re-applies it). ");
        }
        let (stop, boot, load_check) = self.maybe_restart(inst, restart, &info.name, &mut note).await?;
        Ok(DeployReport {
            plugin: info.name,
            version: info.version,
            file: filename,
            replaced,
            backups: new_backups,
            stop,
            boot,
            load_check,
            note,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_check() {
        let ok = "[10:00:00 INFO]: [LuckPerms] Loading server plugin LuckPerms v5.4\n[10:00:01 INFO]: [LuckPerms] Enabling LuckPerms v5.4\n";
        assert!(Manager::check_loaded(ok, "LuckPerms").loaded);
        let bad = "[10:00:01 INFO]: [LuckPerms] Enabling LuckPerms v5.4\n[10:00:02 ERROR]: Error occurred while enabling LuckPerms v5.4 (Is it up to date?)\njava.lang.X\n";
        assert!(!Manager::check_loaded(bad, "LuckPerms").loaded);
        assert!(!Manager::check_loaded("[10:00:00 INFO]: Done", "LuckPerms").loaded);
    }
}
