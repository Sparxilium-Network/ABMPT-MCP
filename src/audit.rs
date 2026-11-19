//! Append-only audit log (JSON lines).

use std::{
    io::Write,
    path::PathBuf,
    sync::Mutex,
};

use serde_json::{Value, json};

use crate::redact::Redactor;

pub struct Audit {
    path: PathBuf,
    lock: Mutex<()>,
    redactor: Redactor,
}

impl Audit {
    pub fn new(path: PathBuf, redactor: Redactor) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
            redactor,
        }
    }

    pub fn record(
        &self,
        agent: &str,
        instance: Option<&str>,
        action: &str,
        detail: Value,
        result: &Result<String, String>,
    ) {
        let (ok, msg) = match result {
            Ok(m) => (true, m.as_str()),
            Err(m) => (false, m.as_str()),
        };
        // JSON results are compacted so the summary carries real content, not just "{".
        let mut summary = match serde_json::from_str::<Value>(msg) {
            Ok(v) if v.is_object() || v.is_array() => v.to_string(),
            _ => msg.lines().next().unwrap_or("").to_string(),
        };
        if summary.len() > 300 {
            let mut end = 300;
            while !summary.is_char_boundary(end) {
                end -= 1;
            }
            summary.truncate(end);
        }
        let line = json!({
            "ts": crate::util::now_rfc3339(),
            "agent": agent,
            "instance": instance,
            "action": action,
            "detail": detail,
            "ok": ok,
            "result": summary,
        });
        let text = self.redactor.redact(&line.to_string());
        let _g = self.lock.lock().unwrap();
        let res = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| writeln!(f, "{text}"));
        if let Err(e) = res {
            tracing::error!("audit write failed: {e}");
        }
    }
}
