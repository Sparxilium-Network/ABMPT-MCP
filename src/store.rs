//! Upload staging area and the server-core jar library.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    jarinfo,
    util::{self, validate_name},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadMeta {
    pub upload_id: String,
    pub filename: String,
    pub size: u64,
    pub sha256: String,
    pub uploaded_by: String,
    pub uploaded_at: String,
}

pub struct Uploads {
    dir: PathBuf,
}

pub const UPLOAD_TTL: Duration = Duration::from_secs(24 * 3600);

/// Accept only simple jar file names.
pub fn sanitize_jar_filename(name: &str) -> Result<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty()
        || base.starts_with('.')
        || !base.to_ascii_lowercase().ends_with(".jar")
        || base.len() > 128
        || !base
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-+ ()[]".contains(c))
    {
        bail!("filename '{name}' is not acceptable: use a plain name ending in .jar");
    }
    Ok(base.to_string())
}

impl Uploads {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn check_id(id: &str) -> Result<()> {
        if id.len() != 32 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("invalid upload_id");
        }
        Ok(())
    }

    pub fn jar_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.jar"))
    }

    pub fn new_id(&self) -> String {
        uuid::Uuid::new_v4().simple().to_string()
    }

    pub fn temp_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!(".{id}.part"))
    }

    pub fn finalize(&self, id: &str, filename: String, uploaded_by: &str) -> Result<UploadMeta> {
        let tmp = self.temp_path(id);
        if !jarinfo::is_zip(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            bail!("uploaded file is not a jar (zip) file");
        }
        let data = std::fs::read(&tmp)?;
        let meta = UploadMeta {
            upload_id: id.to_string(),
            filename,
            size: data.len() as u64,
            sha256: util::sha256_hex(&data),
            uploaded_by: uploaded_by.to_string(),
            uploaded_at: util::now_rfc3339(),
        };
        std::fs::rename(&tmp, self.jar_path(id))?;
        util::write_atomic(
            &self.dir.join(format!("{id}.json")),
            &serde_json::to_vec_pretty(&meta)?,
        )?;
        Ok(meta)
    }

    pub fn get(&self, id: &str) -> Result<(UploadMeta, PathBuf)> {
        Self::check_id(id)?;
        let meta: UploadMeta = std::fs::read(self.dir.join(format!("{id}.json")))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .with_context(|| format!("upload '{id}' not found (uploads expire after 24h)"))?;
        let p = self.jar_path(id);
        if !p.exists() {
            bail!("upload '{id}' not found");
        }
        Ok((meta, p))
    }

    /// Remove expired uploads and stale partial files.
    pub fn cleanup(&self) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let now = SystemTime::now();
        for e in rd.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .map(|m| now.duration_since(m).unwrap_or_default() > UPLOAD_TTL)
                .unwrap_or(false);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreJar {
    pub name: String,
    pub original_filename: String,
    pub label: Option<String>,
    pub size: u64,
    pub sha256: String,
    pub imported_by: String,
    pub imported_at: String,
}

pub struct CoreJars {
    dir: PathBuf,
}

impl CoreJars {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.jar"))
    }

    fn meta_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    pub fn exists(&self, name: &str) -> bool {
        validate_name("core jar", name).is_ok() && self.path(name).exists()
    }

    pub fn import(&self, name: &str, src: &Path, up: &UploadMeta, by: &str) -> Result<CoreJar> {
        validate_name("core jar", name)?;
        if self.path(name).exists() {
            bail!("core jar '{name}' already exists");
        }
        std::fs::copy(src, self.path(name))?;
        let meta = CoreJar {
            name: name.to_string(),
            original_filename: up.filename.clone(),
            label: jarinfo::server_jar_label(&self.path(name)),
            size: up.size,
            sha256: up.sha256.clone(),
            imported_by: by.to_string(),
            imported_at: util::now_rfc3339(),
        };
        util::write_atomic(&self.meta_path(name), &serde_json::to_vec_pretty(&meta)?)?;
        Ok(meta)
    }

    pub fn list(&self) -> Vec<CoreJar> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                if e.path().extension().is_some_and(|x| x == "json")
                    && let Ok(b) = std::fs::read(e.path())
                    && let Ok(m) = serde_json::from_slice::<CoreJar>(&b)
                {
                    out.push(m);
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        validate_name("core jar", name)?;
        if !self.path(name).exists() {
            bail!("core jar '{name}' not found");
        }
        std::fs::remove_file(self.path(name))?;
        let _ = std::fs::remove_file(self.meta_path(name));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames() {
        assert_eq!(
            sanitize_jar_filename("LuckPerms-Bukkit-5.4.1.jar").unwrap(),
            "LuckPerms-Bukkit-5.4.1.jar"
        );
        assert_eq!(sanitize_jar_filename("/tmp/x/a.jar").unwrap(), "a.jar");
        assert!(sanitize_jar_filename("a.sh").is_err());
        assert!(sanitize_jar_filename(".hidden.jar").is_err());
        assert!(sanitize_jar_filename("a;rm.jar").is_err());
    }
}
