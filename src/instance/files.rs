//! Listing, reading and writing files inside an instance folder.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
    time::SystemTime,
};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::{Instance, MANAGED_PROPS, Manager};
use crate::{fsguard, mask, props, util};

pub const CONFIG_EXTS: &[&str] = &[
    "yml", "yaml", "json", "json5", "properties", "toml", "conf", "hocon", "cfg", "ini", "txt",
    "xml", "lang", "csv",
];
const MAX_READ: u64 = 512 * 1024;
const MAX_WRITE: usize = 1024 * 1024;
const MAX_LIST: usize = 500;

fn is_heavy_dir(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    matches!(
        name,
        "logs" | "cache" | "libraries" | "versions" | "crash-reports" | ".paper-remapped" | "bundler" | "debug"
    ) || path.join("level.dat").exists()
}

fn is_config_file(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| CONFIG_EXTS.contains(&x.to_ascii_lowercase().as_str()))
}

/// Relative path -> (size, mtime) of config-like files, skipping worlds/logs/caches.
pub fn config_snapshot(root: &Path) -> BTreeMap<String, (u64, Option<SystemTime>)> {
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut BTreeMap<String, (u64, Option<SystemTime>)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < 8 && !is_heavy_dir(&p) {
                    walk(root, &p, depth + 1, out);
                }
            } else if ft.is_file() && is_config_file(&p) {
                let md = e.metadata().ok();
                out.insert(
                    fsguard::rel_display(root, &p),
                    (md.as_ref().map(|m| m.len()).unwrap_or(0), md.and_then(|m| m.modified().ok())),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    walk(&root, &root, 0, &mut out);
    out
}

#[derive(Debug, Serialize)]
pub struct FileListing {
    pub path: String,
    pub entries: Vec<String>,
    pub truncated: bool,
}

impl Manager {
    pub fn files_list(&self, inst: &Arc<Instance>, rel: &str, depth: usize) -> Result<FileListing> {
        let root = self.dir(&inst.name);
        let start = fsguard::resolve(&root, rel)?;
        if !start.is_dir() {
            bail!("'{rel}' is not a folder");
        }
        let root_c = root.canonicalize()?;
        let mut entries = Vec::new();
        let mut truncated = false;
        fn walk(
            root: &Path,
            dir: &Path,
            depth: usize,
            max_depth: usize,
            out: &mut Vec<String>,
            truncated: &mut bool,
        ) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            let mut items: Vec<_> = rd.flatten().collect();
            items.sort_by_key(|e| e.file_name());
            for e in items {
                if out.len() >= MAX_LIST {
                    *truncated = true;
                    return;
                }
                let p = e.path();
                let Ok(ft) = e.file_type() else { continue };
                let rel = fsguard::rel_display(root, &p);
                if ft.is_dir() {
                    let heavy = is_heavy_dir(&p);
                    out.push(format!("{rel}/{}", if heavy { "  (not expanded)" } else { "" }));
                    if depth + 1 < max_depth && !heavy {
                        walk(root, &p, depth + 1, max_depth, out, truncated);
                    }
                } else if ft.is_file() {
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    out.push(format!("{rel}  ({size} bytes)"));
                }
            }
        }
        walk(&root_c, &start, 0, depth.clamp(1, 5), &mut entries, &mut truncated);
        Ok(FileListing {
            path: fsguard::rel_display(&root_c, &start),
            entries,
            truncated,
        })
    }

    fn read_text(path: &Path, rel: &str) -> Result<String> {
        let md = std::fs::metadata(path).with_context(|| format!("'{rel}' not found"))?;
        if !md.is_file() {
            bail!("'{rel}' is not a regular file");
        }
        if md.len() > MAX_READ {
            bail!("'{rel}' is {} bytes; only files up to {MAX_READ} bytes can be read", md.len());
        }
        let b = std::fs::read(path)?;
        String::from_utf8(b).map_err(|_| anyhow::anyhow!("'{rel}' is not a UTF-8 text file"))
    }

    pub fn file_read(&self, inst: &Arc<Instance>, rel: &str) -> Result<String> {
        let root = self.dir(&inst.name);
        let p = fsguard::resolve(&root, rel)?;
        let text = Self::read_text(&p, rel)?;
        let masked = mask::mask(&text);
        let n = masked.matches(mask::PLACEHOLDER).count();
        let header = if n > 0 {
            format!(
                "# [abmpt] {n} sensitive value(s) shown as {}. Leave those lines unchanged when writing to keep the stored value, or replace them with a real new value.\n",
                mask::PLACEHOLDER
            )
        } else {
            String::new()
        };
        Ok(self.redactor.redact(&format!("{header}{masked}")))
    }

    fn check_writable(rel: &str, path: &Path) -> Result<()> {
        if !is_config_file(path) {
            bail!(
                "'{rel}': only text config files can be written ({}); use the plugin tools for jars",
                CONFIG_EXTS.join(", ")
            );
        }
        Ok(())
    }

    fn validate_managed(&self, inst: &Instance, rel_norm: &str, old: &str, new: &str) -> Result<()> {
        if rel_norm != "server.properties" {
            return Ok(());
        }
        let _ = inst;
        for k in MANAGED_PROPS {
            if props::get(old, k) != props::get(new, k) {
                bail!("'{k}' in server.properties is managed by the MCP service and cannot be changed");
            }
        }
        Ok(())
    }

    fn backup_file(&self, inst: &Instance, rel_norm: &str, path: &Path) -> Result<Option<String>> {
        if !path.exists() {
            return Ok(None);
        }
        let dest = self
            .paths
            .backups_dir(&inst.name)
            .join("files")
            .join(util::stamp())
            .join(rel_norm);
        std::fs::create_dir_all(dest.parent().context("bad backup path")?)?;
        std::fs::copy(path, &dest)?;
        Ok(Some(
            dest.strip_prefix(&self.paths.root)
                .unwrap_or(&dest)
                .to_string_lossy()
                .replace('\\', "/"),
        ))
    }

    fn write_checked(&self, inst: &Arc<Instance>, rel: &str, masked_new: &str) -> Result<String> {
        if masked_new.len() > MAX_WRITE {
            bail!("content too large (max {MAX_WRITE} bytes)");
        }
        let root = self.dir(&inst.name);
        let p = fsguard::resolve(&root, rel)?;
        Self::check_writable(rel, &p)?;
        if p.is_dir() {
            bail!("'{rel}' is a folder");
        }
        let rel_norm = fsguard::rel_display(&root, &p);
        let old = if p.exists() { Self::read_text(&p, rel)? } else { String::new() };
        let new = mask::restore(masked_new, &old)?;
        self.validate_managed(inst, &rel_norm, &old, &new)?;
        if new == old && p.exists() {
            return Ok(format!("'{rel_norm}' unchanged (content identical)"));
        }
        let backup = self.backup_file(inst, &rel_norm, &p)?;
        util::write_atomic(&p, new.as_bytes())?;
        Ok(match backup {
            Some(b) => format!("wrote '{rel_norm}' ({} bytes); previous version backed up to {b}", new.len()),
            None => format!("created '{rel_norm}' ({} bytes)", new.len()),
        })
    }

    pub fn file_write(&self, inst: &Arc<Instance>, rel: &str, content: &str) -> Result<String> {
        self.write_checked(inst, rel, content)
    }

    /// Last `lines` log entries of `logs/latest.log`, optionally filtered.
    pub fn logs_tail(
        &self,
        inst: &Arc<Instance>,
        lines: usize,
        min_level: crate::logs::Level,
        plugin: Option<&str>,
        contains: Option<&str>,
    ) -> Result<String> {
        use std::io::{Read, Seek, SeekFrom};
        const WINDOW: u64 = 4 * 1024 * 1024;
        let p = self.dir(&inst.name).join("logs").join("latest.log");
        let text = if p.exists() {
            let mut f = std::fs::File::open(&p)?;
            let len = f.metadata()?.len();
            let start = len.saturating_sub(WINDOW);
            f.seek(SeekFrom::Start(start))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            let s = String::from_utf8_lossy(&buf).into_owned();
            if start > 0 {
                s.split_once('\n').map(|(_, r)| r.to_string()).unwrap_or(s)
            } else {
                s
            }
        } else if let Some(proc) = inst.proc() {
            proc.tail(300).join("\n")
        } else {
            bail!("instance '{}' has no log yet (it has never been started)", inst.name);
        };
        let filter = crate::logs::Filter { min_level, plugin, contains };
        let entries = crate::logs::filter(crate::logs::entries(&text), &filter);
        let n = lines.clamp(1, 1000);
        let total = entries.len();
        let picked: Vec<String> = entries
            .iter()
            .skip(total.saturating_sub(n))
            .map(|e| crate::logs::shorten(e, 20))
            .collect();
        let body = if picked.is_empty() {
            "(no matching log entries)".to_string()
        } else {
            picked.join("\n")
        };
        let out = format!(
            "# logs/latest.log: showing {} of {} matching entries\n{}",
            picked.len(),
            total,
            body
        );
        Ok(self.redactor.redact(&util::truncate_tail(&out, 48 * 1024)))
    }

    /// Replace one exact occurrence of `old` in the masked view of the file.
    pub fn file_edit(&self, inst: &Arc<Instance>, rel: &str, old: &str, new: &str) -> Result<String> {
        if old.is_empty() {
            bail!("old_string must not be empty");
        }
        let root = self.dir(&inst.name);
        let p = fsguard::resolve(&root, rel)?;
        let current = Self::read_text(&p, rel)?;
        let masked = mask::mask(&current);
        let n = masked.matches(old).count();
        if n != 1 {
            bail!("old_string must match exactly once in '{rel}' (as shown by file_read), found {n} matches");
        }
        let updated = masked.replacen(old, new, 1);
        self.write_checked(inst, rel, &updated)
    }
}
