//! Self-signed TLS for the console listener.
//!
//! The image is a headless appliance reached from another machine on the LAN,
//! so the console key and every chat would otherwise cross the network in the
//! clear. When `CAMEO_TLS_DIR` (or `--tls-dir`) names a directory, the daemon
//! serves HTTPS with the certificate found there, minting a self-signed one on
//! first start if the directory is empty. The SHA-256 fingerprint is printed at
//! startup and by `cameo-hello` so the operator can compare it against the
//! browser's one-time warning — the same trust-on-first-use model Proxmox uses.
//!
//! No certificate authority, no ACME, no client certificates: a LAN appliance
//! with no public name cannot obtain a publicly trusted certificate, and this
//! module does not pretend to. Mesh node identity remains the pairing
//! credential; the certificate only protects the wire.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};

/// A ready server configuration plus what the operator needs to verify it.
pub struct Configured {
    pub config: Arc<rustls::ServerConfig>,
    /// `AB:CD:…` SHA-256 of the leaf certificate, the format
    /// `openssl x509 -fingerprint -sha256` prints, so the two can be compared.
    pub fingerprint: String,
    /// True when this start created the certificate rather than loading one.
    pub minted: bool,
    pub cert_path: PathBuf,
}

/// Build the TLS configuration, or `None` when TLS is not requested.
///
/// `CAMEO_TLS=off` disables it even when a directory is configured, for a
/// deployment that terminates TLS at its own reverse proxy.
pub fn configure(dir: Option<&Path>, bind_host: &str) -> Result<Option<Configured>> {
    let Some(dir) = dir else {
        return Ok(None);
    };
    if std::env::var("CAMEO_TLS").is_ok_and(|value| value.eq_ignore_ascii_case("off")) {
        return Ok(None);
    }
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let minted = if cert_path.is_file() && key_path.is_file() {
        false
    } else {
        mint(dir, &cert_path, &key_path, bind_host)?;
        true
    };
    let certs = CertificateDer::pem_file_iter(&cert_path)
        .with_context(|| format!("reading TLS certificate {}", cert_path.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("parsing TLS certificate {}", cert_path.display()))?;
    let key = PrivateKeyDer::from_pem_file(&key_path)
        .with_context(|| format!("reading TLS private key {}", key_path.display()))?;
    let leaf = certs
        .first()
        .ok_or_else(|| anyhow!("no certificate found in {}", cert_path.display()))?;
    let fingerprint = fingerprint(leaf);
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|error| anyhow!("TLS certificate/key mismatch or unsupported key: {error}"))?;
    Ok(Some(Configured {
        config: Arc::new(config),
        fingerprint,
        minted,
        cert_path,
    }))
}

/// Colon-separated uppercase SHA-256 of the DER certificate.
pub fn fingerprint(certificate: &CertificateDer<'_>) -> String {
    let digest = Sha256::digest(certificate.as_ref());
    digest
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Names the certificate is valid for. `cameo.local` is what mDNS advertises,
/// the machine's own hostname covers `<hostname>.local`, and loopback covers
/// the updater's health probe. A concrete bind address is added when given.
fn subject_names(bind_host: &str) -> Vec<String> {
    let mut names = vec![
        "cameo.local".to_string(),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ];
    #[cfg(unix)]
    if let Ok(hostname) = std::fs::read_to_string("/etc/hostname") {
        let hostname = hostname.trim();
        if !hostname.is_empty()
            && hostname
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            names.push(hostname.to_string());
            names.push(format!("{hostname}.local"));
        }
    }
    let wildcard = matches!(bind_host, "0.0.0.0" | "::" | "");
    if !wildcard && !names.iter().any(|name| name == bind_host) {
        names.push(bind_host.to_string());
    }
    names
}

fn mint(dir: &Path, cert_path: &Path, key_path: &Path, bind_host: &str) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating TLS directory {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("securing TLS directory {}", dir.display()))?;
    }
    let certified = rcgen::generate_simple_self_signed(subject_names(bind_host))
        .map_err(|error| anyhow!("generating a self-signed certificate: {error}"))?;
    write_atomic(
        key_path,
        certified.key_pair.serialize_pem().as_bytes(),
        0o600,
    )?;
    write_atomic(cert_path, certified.cert.pem().as_bytes(), 0o644)?;
    Ok(())
}

/// Create-then-rename so a crash never leaves a half-written key, with the
/// final mode set before the file becomes visible under its real name.
fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    use std::io::Write;
    let temporary = path.with_extension("pem.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("creating {}", temporary.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path).with_context(|| format!("installing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_once_then_reloads_the_same_certificate() {
        let dir = std::env::temp_dir().join(format!("cameo-tls-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = configure(Some(&dir), "0.0.0.0").unwrap().unwrap();
        assert!(first.minted);
        assert_eq!(first.fingerprint.len(), 32 * 3 - 1);
        let second = configure(Some(&dir), "0.0.0.0").unwrap().unwrap();
        assert!(!second.minted);
        assert_eq!(first.fingerprint, second.fingerprint);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_directory_means_plain_http() {
        assert!(configure(None, "127.0.0.1").unwrap().is_none());
    }

    #[test]
    fn subject_names_include_the_bind_address_but_not_wildcards() {
        let names = subject_names("192.168.4.20");
        assert!(names.iter().any(|n| n == "192.168.4.20"));
        assert!(names.iter().any(|n| n == "cameo.local"));
        assert!(!subject_names("0.0.0.0").iter().any(|n| n == "0.0.0.0"));
    }
}
