//! Inspect plugin / server jars.

use std::{io::Read, path::Path};

use anyhow::{Context, Result, bail};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: Option<String>,
    /// `plugin.yml` or `paper-plugin.yml`
    pub descriptor: String,
}

/// Read the plugin descriptor from a jar. Paper plugins take precedence.
pub fn plugin_info(path: &Path) -> Result<PluginInfo> {
    let f = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(f).context("not a valid jar/zip file")?;
    for name in ["paper-plugin.yml", "plugin.yml", "bungee.yml"] {
        let Ok(mut entry) = zip.by_name(name) else {
            continue;
        };
        if entry.size() > 1024 * 1024 {
            bail!("{name} is unreasonably large");
        }
        let mut s = String::new();
        entry.read_to_string(&mut s)?;
        let v: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&s).with_context(|| format!("invalid {name}"))?;
        let pname = v
            .get("name")
            .and_then(|n| n.as_str())
            .with_context(|| format!("{name} has no 'name'"))?
            .to_string();
        if pname.is_empty() || pname.contains(['/', '\\']) || pname.contains("..") {
            bail!("plugin name '{pname}' is not acceptable");
        }
        let version = v.get("version").map(|x| match x {
            serde_yaml_ng::Value::String(s) => s.clone(),
            other => serde_yaml_ng::to_string(other).unwrap_or_default().trim().to_string(),
        });
        return Ok(PluginInfo {
            name: pname,
            version,
            descriptor: name.to_string(),
        });
    }
    bail!("jar has no plugin.yml or paper-plugin.yml; it is not a Bukkit/Paper plugin")
}

/// Very light sanity check that a file is a zip/jar.
pub fn is_zip(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map(|_| magic == *b"PK\x03\x04")
        .unwrap_or(false)
}

/// Describe a server core jar (Paper/Folia) for listing purposes.
pub fn server_jar_label(path: &Path) -> Option<String> {
    let f = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(f).ok()?;
    let mut s = String::new();
    zip.by_name("META-INF/MANIFEST.MF").ok()?.read_to_string(&mut s).ok()?;
    let get = |k: &str| {
        s.lines()
            .find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()))
    };
    let title = get("Implementation-Title:").or_else(|| get("Main-Class:"));
    let ver = get("Implementation-Version:");
    match (title, ver) {
        (Some(t), Some(v)) => Some(format!("{t} {v}")),
        (Some(t), None) => Some(t),
        (None, v) => v,
    }
}

#[cfg(test)]
pub mod testutil {
    use std::io::Write;

    pub fn make_plugin_jar(path: &std::path::Path, name: &str, version: &str) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        z.start_file("plugin.yml", zip::write::SimpleFileOptions::default())
            .unwrap();
        write!(z, "name: {name}\nversion: '{version}'\nmain: a.B\n").unwrap();
        z.finish().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_plugin_yml() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("lp.jar");
        testutil::make_plugin_jar(&p, "LuckPerms", "5.4.1");
        assert!(is_zip(&p));
        let i = plugin_info(&p).unwrap();
        assert_eq!(i.name, "LuckPerms");
        assert_eq!(i.version.as_deref(), Some("5.4.1"));
    }
}
