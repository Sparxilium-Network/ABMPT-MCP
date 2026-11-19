//! Log parsing and filtering for Paper/Folia console logs.

use std::sync::LazyLock;

use regex::Regex;

// `[12:34:56 ERROR]: msg` (console) or `[12:34:56] [Server thread/ERROR]: msg` (latest.log)
static HEAD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[(?:\d{2}:\d{2}:\d{2} (?P<l1>[A-Z]+)|\d{2}:\d{2}:\d{2}\] \[[^\]]*/(?P<l2>[A-Z]+))\]:? ?(?P<msg>.*)$")
        .unwrap()
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn parse(s: &str) -> Self {
        match s {
            "ERROR" | "FATAL" | "SEVERE" => Level::Error,
            "WARN" | "WARNING" => Level::Warn,
            "DEBUG" | "TRACE" => Level::Debug,
            _ => Level::Info,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub level: Level,
    pub text: String,
}

impl Entry {
    /// `[Plugin]` tag at the start of the message, if any.
    pub fn plugin_tag(&self) -> Option<&str> {
        let first = self.text.lines().next()?;
        let c = HEAD_RE.captures(first)?;
        let msg = c.name("msg")?.as_str();
        let rest = msg.strip_prefix('[')?;
        rest.split_once(']').map(|(t, _)| t)
    }
}

/// Is this line the start of a new log entry?
pub fn line_level(line: &str) -> Option<Level> {
    let c = HEAD_RE.captures(line)?;
    let l = c.name("l1").or(c.name("l2"))?.as_str();
    Some(Level::parse(l))
}

/// Group lines into entries; continuation lines (stack traces) join the previous entry.
pub fn entries(text: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for raw in text.lines() {
        let line = crate::util::strip_colors(raw);
        match line_level(&line) {
            Some(level) => out.push(Entry { level, text: line }),
            None => match out.last_mut() {
                Some(e) => {
                    e.text.push('\n');
                    e.text.push_str(&line);
                }
                None => out.push(Entry {
                    level: Level::Info,
                    text: line,
                }),
            },
        }
    }
    out
}

pub struct Filter<'a> {
    pub min_level: Level,
    pub plugin: Option<&'a str>,
    pub contains: Option<&'a str>,
}

pub fn filter(entries: Vec<Entry>, f: &Filter<'_>) -> Vec<Entry> {
    entries
        .into_iter()
        .filter(|e| e.level >= f.min_level)
        .filter(|e| match f.plugin {
            None => true,
            Some(p) => {
                e.plugin_tag().is_some_and(|t| t.eq_ignore_ascii_case(p))
                    || e.text.lines().next().is_some_and(|l| {
                        l.to_ascii_lowercase().contains(&p.to_ascii_lowercase())
                    })
            }
        })
        .filter(|e| match f.contains {
            None => true,
            Some(s) => e.text.to_lowercase().contains(&s.to_lowercase()),
        })
        .collect()
}

/// Cap a long stack trace to its first `max_lines` lines.
pub fn shorten(e: &Entry, max_lines: usize) -> String {
    let lines: Vec<&str> = e.text.lines().collect();
    if lines.len() <= max_lines {
        return e.text.clone();
    }
    format!(
        "{}\n\t… ({} more lines)",
        lines[..max_lines].join("\n"),
        lines.len() - max_lines
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "[10:00:00] [Server thread/INFO]: Starting minecraft server\n\
[10:00:01] [Server thread/ERROR]: [LuckPerms] Unable to connect\n\
java.sql.SQLException: boom\n\
\tat x.y(Z.java:1)\n\
[10:00:02] [Server thread/WARN]: [Essentials] slow\n\
[10:00:03 INFO]: [LuckPerms] Enabling LuckPerms v5.4\n";

    #[test]
    fn groups_and_filters() {
        let e = entries(LOG);
        assert_eq!(e.len(), 4);
        assert_eq!(e[1].level, Level::Error);
        assert!(e[1].text.contains("SQLException"));
        assert_eq!(e[1].plugin_tag(), Some("LuckPerms"));

        let errs = filter(
            e.clone(),
            &Filter { min_level: Level::Error, plugin: None, contains: None },
        );
        assert_eq!(errs.len(), 1);
        let lp = filter(
            e,
            &Filter { min_level: Level::Debug, plugin: Some("luckperms"), contains: None },
        );
        assert_eq!(lp.len(), 2);
    }
}
