use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use russh::client;
use russh::keys::{self, HashAlg, PublicKey, PublicKeyOrCertificate};

use crate::error::Error;

pub fn default_path(home: &Path) -> PathBuf {
    home.join(".config").join("sshhh").join("known_hosts")
}

pub fn host_pattern(host: &str, port: u16) -> String {
    if port == 22 { host.to_string() } else { format!("[{host}]:{port}") }
}

pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

pub fn ensure_parent(path: &Path) -> Result<(), Error> {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => {
            std::fs::create_dir_all(dir).map_err(|e| Error::Io(format!("{}: {}", dir.display(), e.kind())))
        }
        _ => Ok(()),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Known {
    Matches,
    Unknown,
}

pub fn check(path: &Path, host: &str, port: u16, key: &PublicKey) -> Result<Known, Error> {
    match keys::known_hosts::check_known_hosts_path(host, port, key, path) {
        Ok(true) => Ok(Known::Matches),
        Ok(false) => Ok(Known::Unknown),
        Err(keys::Error::KeyChanged { line }) => {
            Err(Error::HostKeyChanged { host: host.into(), port, fingerprint: fingerprint(key), line })
        }
        Err(e) => Err(Error::HostKeyUnusable { host: host.into(), port, reason: e.to_string() }),
    }
}

pub fn learn(path: &Path, host: &str, port: u16, key: &PublicKey) -> Result<(), Error> {
    ensure_parent(path)?;
    keys::known_hosts::learn_known_hosts_path(host, port, key, path)
        .map_err(|e| Error::HostKeyUnusable { host: host.into(), port, reason: e.to_string() })
}

struct Probe {
    seen: Arc<Mutex<Option<PublicKey>>>,
}

impl client::Handler for Probe {
    type Error = russh::Error;

    async fn check_server_key(&mut self, presented: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        if let PublicKeyOrCertificate::PublicKey { key, .. } = presented {
            *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(key.clone());
        }
        Ok(false)
    }
}

/// Reads the server's host key during the handshake and disconnects before any authentication.
pub async fn probe(host: &str, port: u16, timeout: Duration) -> Result<PublicKey, Error> {
    let seen = Arc::new(Mutex::new(None));
    let config = Arc::new(client::Config::default());
    let attempt = client::connect(config, (host, port), Probe { seen: seen.clone() });
    let outcome = tokio::time::timeout(timeout, attempt)
        .await
        .map_err(|_| Error::ConnectTimeout { host: host.into(), port, secs: timeout.as_secs() })?;
    let captured = seen.lock().unwrap_or_else(PoisonError::into_inner).take();
    match (captured, outcome) {
        (Some(key), _) => Ok(key),
        (None, Err(e)) => Err(Error::Connect { host: host.into(), port, reason: e.to_string() }),
        (None, Ok(_)) => Err(Error::HostKeyUnusable { host: host.into(), port, reason: "the server presented no plain host key".into() }),
    }
}

/// Removes every plain entry for `host:port`; hashed entries cannot be matched and are left alone.
pub fn forget(path: &Path, host: &str, port: u16) -> Result<usize, Error> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(Error::Io(format!("{}: {}", path.display(), e.kind()))),
    };
    let pattern = host_pattern(host, port);
    let mut kept = String::with_capacity(text.len());
    let mut removed = 0;
    for line in text.split_inclusive('\n') {
        let mut fields = line.split_whitespace();
        let first = fields.next().filter(|f| !f.starts_with('#'));
        let hosts = match first {
            Some(marker) if marker.starts_with('@') => fields.next(),
            other => other,
        };
        let Some(hosts) = hosts.filter(|h| h.split(',').any(|p| p.eq_ignore_ascii_case(&pattern))) else {
            kept.push_str(line);
            continue;
        };
        removed += 1;
        let others: Vec<&str> = hosts.split(',').filter(|p| !p.eq_ignore_ascii_case(&pattern)).collect();
        if !others.is_empty() {
            kept.push_str(&line.replacen(hosts, &others.join(","), 1));
        }
    }
    if removed > 0 {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, kept).map_err(|e| Error::Io(format!("{}: {}", tmp.display(), e.kind())))?;
        std::fs::rename(&tmp, path).map_err(|e| Error::Io(format!("{}: {}", path.display(), e.kind())))?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forget_removes_only_the_matching_host_and_port() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::write(
            &path,
            "# kept\nexample.test ssh-ed25519 AAAA1\n[example.test]:2222 ssh-ed25519 AAAA2\n\nother.test,10.0.0.1 ssh-ed25519 AAAA3\n",
        )
        .unwrap();
        assert_eq!(forget(&path, "example.test", 2222).unwrap(), 1);
        let left = std::fs::read_to_string(&path).unwrap();
        assert!(left.contains("AAAA1") && left.contains("AAAA3") && !left.contains("AAAA2") && left.contains("# kept"));
        assert_eq!(forget(&path, "10.0.0.1", 22).unwrap(), 1);
        let left = std::fs::read_to_string(&path).unwrap();
        assert!(left.contains("other.test ssh-ed25519 AAAA3") && !left.contains("10.0.0.1"));
        assert_eq!(forget(&path, "nowhere.test", 22).unwrap(), 0);
        assert_eq!(forget(&dir.path().join("missing"), "x", 22).unwrap(), 0);
    }
}
