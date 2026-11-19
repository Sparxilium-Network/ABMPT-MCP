//! Small shared helpers.

use std::{
    io::Write,
    path::Path,
    sync::LazyLock,
};

use anyhow::{Context, Result, bail};
use regex::Regex;

static NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,31}$").unwrap());

/// Names for instances, jars and tokens: lowercase, digits, `_`, `-`, max 32 chars.
pub fn validate_name(kind: &str, name: &str) -> Result<()> {
    if !NAME_RE.is_match(name) {
        bail!(
            "invalid {kind} name '{name}': use 1-32 chars of a-z, 0-9, '_' or '-', starting with a letter or digit"
        );
    }
    Ok(())
}

/// Write a file atomically (temp file + rename) so a crash never leaves it half written.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("file"),
        uuid::Uuid::new_v4().simple()
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("rename into {}", path.display()))?;
    Ok(())
}

/// Restrict a file to the owner (secrets: tokens, keys, instance metadata).
pub fn set_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    write_atomic(path, data)?;
    set_private(path);
    Ok(())
}

static COLOR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)§[0-9a-fk-orx]|\x1b\[[0-9;?]*[A-Za-z]").unwrap());

/// Remove Minecraft `§` color codes and ANSI escape sequences.
pub fn strip_colors(s: &str) -> String {
    COLOR_RE.replace_all(s, "").into_owned()
}

/// Keep at most `max` bytes, cutting from the front and noting the truncation.
pub fn truncate_tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…(truncated, showing last {max} bytes)\n{}", &s[start..])
}

/// Keep at most `max` bytes, cutting from the end.
pub fn truncate_head(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…(truncated at {max} bytes)", &s[..end])
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Timestamp usable in file names.
pub fn stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string()
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(validate_name("x", "lobby-1").is_ok());
        assert!(validate_name("x", "Lobby").is_err());
        assert!(validate_name("x", "../a").is_err());
        assert!(validate_name("x", "").is_err());
        assert!(validate_name("x", "-a").is_err());
    }

    #[test]
    fn colors() {
        assert_eq!(
            strip_colors("§aThere are §c0§a of §x§f§f§0§0§0§0max"),
            "There are 0 of max"
        );
        assert_eq!(strip_colors("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate_tail("abcdef", 10), "abcdef");
        assert!(truncate_tail("abcdef", 3).ends_with("def"));
        assert!(truncate_head("abcdef", 3).starts_with("abc"));
    }
}
