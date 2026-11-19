//! Last line of defence: scrub known secrets from any text that leaves the server.

use std::{
    collections::HashSet,
    sync::{Arc, LazyLock, RwLock},
};

use regex::Regex;

static TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"abmpt_[0-9a-f]{64}").unwrap());

#[derive(Clone, Default)]
pub struct Redactor {
    secrets: Arc<RwLock<HashSet<String>>>,
}

impl Redactor {
    pub fn add(&self, secret: &str) {
        if secret.len() >= 8 {
            self.secrets.write().unwrap().insert(secret.to_string());
        }
    }

    pub fn remove(&self, secret: &str) {
        self.secrets.write().unwrap().remove(secret);
    }

    pub fn redact(&self, text: &str) -> String {
        let mut out = TOKEN_RE.replace_all(text, "[REDACTED]").into_owned();
        for s in self.secrets.read().unwrap().iter() {
            if out.contains(s.as_str()) {
                out = out.replace(s.as_str(), "[REDACTED]");
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs() {
        let r = Redactor::default();
        r.add("supersecretpw");
        let tok = format!("abmpt_{}", "a".repeat(64));
        let s = r.redact(&format!("pw=supersecretpw tok={tok}"));
        assert_eq!(s, "pw=[REDACTED] tok=[REDACTED]");
    }
}
