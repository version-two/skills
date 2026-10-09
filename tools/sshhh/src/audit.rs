use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::error::Error;
use crate::spec::hex;

pub struct Entry<'a> {
    pub alias: &'a str,
    pub user: &'a str,
    pub op: &'static str,
    pub mechanism: &'a str,
    pub rc: Option<i32>,
    pub error: Option<&'static str>,
    pub duration_ms: u64,
    pub subject: &'a str,
    pub store_subject: bool,
}

pub struct AuditLog {
    file: std::fs::File,
}

pub fn path(home: &Path) -> PathBuf {
    home.join(".local").join("state").join("sshhh").join("audit.jsonl")
}

impl AuditLog {
    /// Opened before anything runs, so a log that cannot be written stops the operation up front.
    pub fn open(home: &Path) -> Result<AuditLog, Error> {
        let path = path(home);
        let fail = |e: std::io::Error| Error::Io(format!("audit log {}: {}", path.display(), e.kind()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        Ok(AuditLog { file: options.open(&path).map_err(fail)? })
    }

    pub fn write(&mut self, entry: &Entry) -> Result<(), String> {
        let mut line = json!({
            "ts": rfc3339(SystemTime::now()),
            "alias": entry.alias,
            "user": entry.user,
            "op": entry.op,
            "mechanism": entry.mechanism,
            "rc": entry.rc,
            "error": entry.error,
            "duration_ms": entry.duration_ms,
            "sha256": hex(&Sha256::digest(entry.subject.as_bytes())),
        });
        if entry.store_subject {
            line["subject"] = json!(entry.subject);
        }
        writeln!(self.file, "{line}").map_err(|e| format!("audit log write failed: {}", e.kind()))
    }
}

fn rfc3339(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3_600, rem % 3_600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_utc_rfc3339() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(UNIX_EPOCH + Duration::from_secs(1_709_164_800)), "2024-02-29T00:00:00Z");
        assert_eq!(rfc3339(UNIX_EPOCH + Duration::from_secs(1_791_540_615)), "2026-10-09T10:10:15Z");
    }

    #[test]
    fn entries_hash_the_subject_and_store_it_only_on_request() {
        let home = tempfile::tempdir().unwrap();
        let mut log = AuditLog::open(home.path()).unwrap();
        let entry = |store| Entry {
            alias: "zeus",
            user: "deploy",
            op: "run",
            mechanism: "sudo",
            rc: Some(0),
            error: None,
            duration_ms: 12,
            subject: "uptime",
            store_subject: store,
        };
        log.write(&entry(false)).unwrap();
        log.write(&entry(true)).unwrap();
        let text = std::fs::read_to_string(path(home.path())).unwrap();
        let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].get("subject").is_none());
        assert_eq!(lines[1]["subject"], "uptime");
        assert_eq!(lines[0]["sha256"].as_str().unwrap().len(), 64);
        assert_eq!((lines[0]["alias"].as_str(), lines[0]["mechanism"].as_str(), lines[0]["rc"].as_i64()), (Some("zeus"), Some("sudo"), Some(0)));
    }
}
