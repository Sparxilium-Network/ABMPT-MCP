//! Per-agent bearer tokens. Only SHA-256 hashes are stored; the plaintext is shown once.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use anyhow::{Result, bail};
use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

use crate::util::{self, sha256_hex, validate_name};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenEntry {
    pub name: String,
    pub sha256: String,
    pub created: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TokenFile {
    tokens: Vec<TokenEntry>,
}

/// Identity of the calling agent, attached to each authenticated request.
#[derive(Debug, Clone)]
pub struct Agent(pub String);

pub struct TokenStore {
    path: PathBuf,
    /// Cached file contents, keyed by mtime so CLI changes (e.g. revocation) apply immediately.
    cache: Mutex<Option<(SystemTime, Vec<TokenEntry>)>>,
}

impl TokenStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cache: Mutex::new(None),
        }
    }

    fn load(&self) -> Result<TokenFile> {
        match std::fs::read(&self.path) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TokenFile::default()),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, f: &TokenFile) -> Result<()> {
        util::write_private(&self.path, &serde_json::to_vec_pretty(f)?)
    }

    /// Create a token for `name` and return the plaintext (only time it is ever visible).
    pub fn create(&self, name: &str) -> Result<String> {
        validate_name("token", name)?;
        let mut f = self.load()?;
        if f.tokens.iter().any(|t| t.name == name) {
            bail!("token '{name}' already exists; revoke it first to rotate");
        }
        let mut raw = [0u8; 32];
        rand::rng().fill_bytes(&mut raw);
        let token = format!("abmpt_{}", hex::encode(raw));
        f.tokens.push(TokenEntry {
            name: name.to_string(),
            sha256: sha256_hex(token.as_bytes()),
            created: util::now_rfc3339(),
        });
        self.save(&f)?;
        Ok(token)
    }

    pub fn revoke(&self, name: &str) -> Result<()> {
        let mut f = self.load()?;
        let before = f.tokens.len();
        f.tokens.retain(|t| t.name != name);
        if f.tokens.len() == before {
            bail!("no token named '{name}'");
        }
        self.save(&f)
    }

    pub fn list(&self) -> Result<Vec<TokenEntry>> {
        Ok(self.load()?.tokens)
    }

    fn entries(&self) -> Vec<TokenEntry> {
        let mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        let mut cache = self.cache.lock().unwrap();
        match (mtime, cache.as_ref()) {
            (Some(m), Some((cm, entries))) if *cm == m => entries.clone(),
            (Some(m), _) => {
                let entries = self.load().map(|f| f.tokens).unwrap_or_else(|e| {
                    tracing::error!("cannot read token file: {e:#}");
                    Vec::new()
                });
                *cache = Some((m, entries.clone()));
                entries
            }
            (None, _) => {
                *cache = None;
                Vec::new()
            }
        }
    }

    /// Returns the agent name if the token is valid.
    pub fn verify(&self, token: &str) -> Option<String> {
        let h = sha256_hex(token.as_bytes());
        let mut found = None;
        for e in self.entries() {
            if bool::from(e.sha256.as_bytes().ct_eq(h.as_bytes())) {
                found = Some(e.name);
            }
        }
        found
    }
}

pub async fn require_token(
    State(store): State<Arc<TokenStore>>,
    mut req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(str::trim);
    let Some(agent) = token.and_then(|t| store.verify(t)) else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "missing or invalid bearer token",
        )
            .into_response();
    };
    req.extensions_mut().insert(Agent(agent));
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_verify_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::new(dir.path().join("tokens.json"));
        let t1 = store.create("claude").unwrap();
        let t2 = store.create("codex").unwrap();
        assert!(store.create("claude").is_err());
        assert_eq!(store.verify(&t1).as_deref(), Some("claude"));
        assert_eq!(store.verify(&t2).as_deref(), Some("codex"));
        assert_eq!(store.verify("abmpt_nope"), None);
        // the plaintext is never stored
        let raw = std::fs::read_to_string(dir.path().join("tokens.json")).unwrap();
        assert!(!raw.contains(&t1));
        store.revoke("claude").unwrap();
        // force mtime-based cache refresh even on coarse filesystems
        *store.cache.lock().unwrap() = None;
        assert_eq!(store.verify(&t1), None);
        assert_eq!(store.verify(&t2).as_deref(), Some("codex"));
    }
}
