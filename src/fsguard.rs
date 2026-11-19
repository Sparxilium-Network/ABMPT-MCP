//! Resolve caller-supplied relative paths strictly inside an instance directory.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Resolve `rel` under `root`. Rejects absolute paths, `..`, and anything that escapes
/// `root` through symlinks. The target itself may not exist yet (for writes), but its
/// parent directory must.
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.trim().replace('\\', "/");
    let rel = rel.trim_start_matches("./");
    let p = Path::new(rel);
    if rel.is_empty() {
        return Ok(root.canonicalize()?);
    }
    for c in p.components() {
        match c {
            Component::Normal(_) => {}
            Component::CurDir => {}
            _ => bail!("path '{rel}' is not allowed: use a path relative to the instance folder without '..'"),
        }
    }
    let root_c = root.canonicalize().context("instance folder missing")?;
    let joined = root_c.join(p);
    let checked = if joined.exists() {
        joined.canonicalize()?
    } else {
        let parent = joined.parent().context("bad path")?;
        let parent_c = parent
            .canonicalize()
            .with_context(|| format!("parent folder of '{rel}' does not exist"))?;
        parent_c.join(joined.file_name().context("bad path")?)
    };
    if !checked.starts_with(&root_c) {
        bail!("path '{rel}' resolves outside the instance folder");
    }
    Ok(checked)
}

/// Relative display path (always `/`-separated).
pub fn rel_display(root: &Path, p: &Path) -> String {
    let root_c = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    p.strip_prefix(&root_c)
        .or_else(|_| p.strip_prefix(root))
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_traversal() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("inst");
        std::fs::create_dir_all(root.join("plugins/LuckPerms")).unwrap();
        std::fs::write(d.path().join("secret.txt"), "x").unwrap();
        assert!(resolve(&root, "../secret.txt").is_err());
        assert!(resolve(&root, "plugins/../../secret.txt").is_err());
        assert!(resolve(&root, "/etc/passwd").is_err());
        assert!(resolve(&root, "C:\\Windows").is_err() || cfg!(unix));
        assert!(resolve(&root, "plugins/LuckPerms/config.yml").is_ok());
        assert!(resolve(&root, "server.properties").is_ok());
        assert!(resolve(&root, "nope/x.yml").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn blocks_symlink_escape() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("inst");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(d.path(), root.join("link")).unwrap();
        std::fs::write(d.path().join("secret.txt"), "x").unwrap();
        assert!(resolve(&root, "link/secret.txt").is_err());
    }
}
