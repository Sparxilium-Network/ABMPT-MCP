//! Local CA + server certificate, generated on first start and kept in the data volume.

use std::{net::IpAddr, path::Path};

use anyhow::{Context, Result};
use chrono::Datelike;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
    IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType, date_time_ymd,
};
use serde::{Deserialize, Serialize};

use crate::util;

/// Bump when the server certificate profile changes, to force re-issuing.
const CERT_PROFILE: u32 = 2;

#[derive(Serialize, Deserialize, PartialEq)]
struct ServerCertMeta {
    #[serde(default)]
    profile: u32,
    sans: Vec<String>,
    /// Year/month/day after which the certificate should be renewed.
    renew_after: String,
}

pub struct TlsFiles {
    pub cert: std::path::PathBuf,
    pub key: std::path::PathBuf,
    pub ca_cert: std::path::PathBuf,
}

fn ensure_ca(dir: &Path) -> Result<(String, String)> {
    let crt = dir.join("ca.crt");
    let key = dir.join("ca.key");
    if crt.exists() && key.exists() {
        return Ok((std::fs::read_to_string(&crt)?, std::fs::read_to_string(&key)?));
    }
    tracing::info!("generating local CA in {}", dir.display());
    let kp = KeyPair::generate()?;
    let mut p = CertificateParams::new(Vec::<String>::new())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "ABMPT Local CA");
    dn.push(DnType::OrganizationName, "ABMPT");
    p.distinguished_name = dn;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let y = chrono::Utc::now().year();
    p.not_before = date_time_ymd(y - 1, 1, 1);
    p.not_after = date_time_ymd(y + 20, 1, 1);
    let cert = p.self_signed(&kp)?;
    util::write_private(&key, kp.serialize_pem().as_bytes())?;
    util::write_atomic(&crt, cert.pem().as_bytes())?;
    Ok((cert.pem(), kp.serialize_pem()))
}

/// Make sure CA and server certificate exist and cover `sans`; regenerate the server
/// certificate when the SAN list changes or it is close to expiry. The CA is kept.
pub fn ensure(dir: &Path, sans: &[String]) -> Result<TlsFiles> {
    std::fs::create_dir_all(dir)?;
    let (ca_pem, ca_key_pem) = ensure_ca(dir)?;
    let files = TlsFiles {
        cert: dir.join("server.crt"),
        key: dir.join("server.key"),
        ca_cert: dir.join("ca.crt"),
    };
    let meta_path = dir.join("server.meta.json");
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let current: Option<ServerCertMeta> = std::fs::read(&meta_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(m) = &current
        && m.sans == sans
        && m.profile == CERT_PROFILE
        && m.renew_after > today
        && files.cert.exists()
        && files.key.exists()
    {
        return Ok(files);
    }

    tracing::info!("generating server certificate for {sans:?}");
    let ca_key = KeyPair::from_pem(&ca_key_pem).context("parse ca.key")?;
    let issuer = Issuer::from_ca_cert_pem(&ca_pem, ca_key).context("parse ca.crt")?;
    let kp = KeyPair::generate()?;
    let mut p = CertificateParams::new(Vec::<String>::new())?;
    for s in sans {
        let san = match s.parse::<IpAddr>() {
            Ok(ip) => SanType::IpAddress(ip),
            Err(_) => SanType::DnsName(s.as_str().try_into().context("invalid DNS SAN")?),
        };
        p.subject_alt_names.push(san);
    }
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, sans.first().cloned().unwrap_or_default());
    p.distinguished_name = dn;
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    // Strict verifiers (OpenSSL X509_STRICT, Python >= 3.13) require AKI on leaf certs.
    p.use_authority_key_identifier_extension = true;
    let now = chrono::Utc::now();
    let y = now.year();
    p.not_before = date_time_ymd(y - 1, 1, 1);
    // ~2 years validity; renewed automatically one month before.
    p.not_after = date_time_ymd(y + 2, now.month() as u8, 1);
    let cert = p.signed_by(&kp, &issuer)?;
    util::write_private(&files.key, kp.serialize_pem().as_bytes())?;
    util::write_atomic(&files.cert, cert.pem().as_bytes())?;
    let renew = format!("{:04}-{:02}-01", y + 2, now.month());
    let renew_after = chrono::NaiveDate::parse_from_str(&renew, "%Y-%m-%d")
        .map(|d| (d - chrono::Days::new(31)).format("%Y-%m-%d").to_string())
        .unwrap_or(renew);
    util::write_atomic(
        &meta_path,
        &serde_json::to_vec_pretty(&ServerCertMeta {
            profile: CERT_PROFILE,
            sans: sans.to_vec(),
            renew_after,
        })?,
    )?;
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_and_reuses() {
        let dir = tempfile::tempdir().unwrap();
        let sans = vec!["localhost".to_string(), "192.168.75.206".to_string()];
        let f = ensure(dir.path(), &sans).unwrap();
        let first = std::fs::read_to_string(&f.cert).unwrap();
        let ca = std::fs::read_to_string(&f.ca_cert).unwrap();
        // second call reuses
        let f = ensure(dir.path(), &sans).unwrap();
        assert_eq!(first, std::fs::read_to_string(&f.cert).unwrap());
        // SAN change regenerates server cert but keeps the CA
        let f = ensure(dir.path(), &["localhost".to_string()]).unwrap();
        assert_ne!(first, std::fs::read_to_string(&f.cert).unwrap());
        assert_eq!(ca, std::fs::read_to_string(&f.ca_cert).unwrap());
    }
}
