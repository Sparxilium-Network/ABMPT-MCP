//! Masking of sensitive values in config files, and restoring them on write.
//!
//! Reading replaces sensitive values with [`PLACEHOLDER`]. When the caller writes the file
//! back, any line still containing the placeholder must be identical to a masked line of
//! the current file; it is then swapped back to the original line. To change a secret the
//! caller simply writes the real new value.

use std::{
    collections::{HashMap, VecDeque},
    sync::LazyLock,
};

use anyhow::{Result, bail};
use regex::Regex;

pub const PLACEHOLDER: &str = "__ABMPT_REDACTED__";

static KV_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(\s*(?:-\s*)?["']?)([A-Za-z0-9_.\-]+)(["']?\s*[:=]\s*)(.*?)(,?\s*)$"#).unwrap()
});
static SENSITIVE_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(password|passwd|passphrase|pwd|secret|token|api[-_.]?key|private[-_.]?key|access[-_.]?key|credentials?|^pass$)",
    )
    .unwrap()
});
static URL_USERINFO_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([A-Za-z][A-Za-z0-9+.\-]*://[^:/@\s]+:)([^@\s/]+)(@)").unwrap());

fn is_empty_value(v: &str) -> bool {
    let t = v.trim();
    t.is_empty() || t == "\"\"" || t == "''" || t == "~" || t == "null" || t == "{}" || t == "[]"
}

/// Mask one line. Returns the line unchanged when nothing is sensitive.
pub fn mask_line(line: &str) -> String {
    let mut out = line.to_string();
    if let Some(c) = KV_RE.captures(line)
        && SENSITIVE_KEY_RE.is_match(&c[2])
        && !is_empty_value(&c[4])
        // a YAML/TOML section header like `password:` with nested keys has an empty value
        && !c[4].trim_start().starts_with('#')
    {
        out = format!("{}{}{}{}{}", &c[1], &c[2], &c[3], PLACEHOLDER, &c[5]);
    }
    URL_USERINFO_RE
        .replace_all(&out, format!("${{1}}{PLACEHOLDER}${{3}}"))
        .into_owned()
}

pub fn mask(text: &str) -> String {
    let mut out: Vec<String> = text.lines().map(mask_line).collect();
    if text.ends_with('\n') {
        out.push(String::new());
    }
    out.join("\n")
}

/// Swap placeholder lines in `new_text` back to the original values from `original`.
pub fn restore(new_text: &str, original: &str) -> Result<String> {
    let mut pool: HashMap<String, VecDeque<&str>> = HashMap::new();
    for l in original.lines() {
        let m = mask_line(l);
        if m != l {
            pool.entry(m).or_default().push_back(l);
        }
    }
    let mut out = Vec::new();
    for (i, l) in new_text.lines().enumerate() {
        if l.contains(PLACEHOLDER) {
            match pool.get_mut(l).and_then(|q| q.pop_front()) {
                Some(orig) => out.push(orig.to_string()),
                None => bail!(
                    "line {}: contains {PLACEHOLDER} but does not match a masked line of the current file. \
                     Keep masked lines exactly as read, or write the real value.",
                    i + 1
                ),
            }
        } else {
            out.push(l.to_string());
        }
    }
    let mut s = out.join("\n");
    if new_text.ends_with('\n') {
        s.push('\n');
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LP: &str = "storage-method: h2\ndata:\n  address: localhost\n  password: 'hunter2'\n  username: root\nrcon.password=abc123\ntoken-bucket:\nurl: jdbc:mysql://u:p4ss@db:3306/x\nempty-password: ''\n";

    #[test]
    fn masks_sensitive_values() {
        let m = mask(LP);
        assert!(!m.contains("hunter2"));
        assert!(!m.contains("abc123"));
        assert!(!m.contains("p4ss"));
        assert!(m.contains("  password: __ABMPT_REDACTED__\n"));
        assert!(m.contains("token-bucket:\n"));
        assert!(m.contains("empty-password: ''"));
        assert!(m.contains("username: root"));
        assert!(m.ends_with('\n'));
    }

    #[test]
    fn restores_unchanged_masked_lines() {
        let m = mask(LP);
        let edited = m.replace("storage-method: h2", "storage-method: mysql");
        let r = restore(&edited, LP).unwrap();
        assert_eq!(r, LP.replace("storage-method: h2", "storage-method: mysql"));
    }

    #[test]
    fn allows_writing_new_secret() {
        let m = mask(LP);
        let edited = m.replace("  password: __ABMPT_REDACTED__", "  password: 'newpw'");
        let r = restore(&edited, LP).unwrap();
        assert!(r.contains("password: 'newpw'"));
        assert!(r.contains("rcon.password=abc123"));
    }

    #[test]
    fn rejects_modified_placeholder_line() {
        let m = mask(LP);
        let edited = m.replace("  password: __ABMPT_REDACTED__", "  passwd: __ABMPT_REDACTED__");
        assert!(restore(&edited, LP).is_err());
    }

    #[test]
    fn duplicate_masked_lines_keep_order() {
        let src = "a:\n  password: one\nb:\n  password: two\n";
        let r = restore(&mask(src), src).unwrap();
        assert_eq!(r, src);
    }
}
