use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::Error;
use crate::policy::{self, Access, Policy};
use crate::spec::hex;

const CHUNK: usize = 256 * 1024;
static COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub verify: bool,
    pub private: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransferReport {
    pub op: &'static str,
    pub local: String,
    pub remote: String,
    pub bytes: u64,
    pub sha256: String,
    pub verified: bool,
    pub duration_ms: u64,
    pub warnings: Vec<String>,
}

fn transfer(op: &'static str, path: impl Display, reason: impl Display) -> Error {
    Error::Transfer { op, path: path.to_string(), reason: reason.to_string() }
}

fn token() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let seed = format!("{}-{nanos}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
    hex(&Sha256::digest(seed.as_bytes()))[..10].to_string()
}

fn duration_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

async fn expand(sftp: &SftpSession, path: &str) -> Result<String, Error> {
    if path == "~" || path.starts_with("~/") {
        let home = sftp.canonicalize(".").await.map_err(|e| transfer("resolve", path, e))?;
        return Ok(format!("{}{}", home.trim_end_matches('/'), &path[1..]));
    }
    Ok(path.to_string())
}

async fn remote_is_dir(sftp: &SftpSession, path: &str) -> Result<Option<bool>, Error> {
    if !sftp.try_exists(path).await.map_err(|e| transfer("stat", path, e))? {
        return Ok(None);
    }
    let meta = sftp.metadata(path).await.map_err(|e| transfer("stat", path, e))?;
    Ok(Some(meta.file_type().is_dir()))
}

/// Checks `path` against the path rules both as written and as the server resolves it, so a
/// symlink or `..` cannot lead out of the allowed area. The resolved path is checked in addition,
/// never instead.
async fn guard(sftp: &SftpSession, policy: &Policy, path: &str, access: Access) -> Result<(), Error> {
    if !policy.path_policy() {
        return Ok(());
    }
    let absolute = if path.starts_with('/') {
        path.to_string()
    } else {
        let here = sftp.canonicalize(".").await.map_err(|e| transfer("resolve", path, e))?;
        format!("{}/{path}", here.trim_end_matches('/'))
    };
    policy.check_path(&absolute, access)?;
    let segments = policy::normalize(&absolute).ok_or_else(|| transfer("resolve", path, "not an absolute path"))?;
    let lexical = format!("/{}", segments.join("/"));
    let real = if sftp.try_exists(lexical.as_str()).await.map_err(|e| transfer("stat", &lexical, e))? {
        sftp.canonicalize(lexical.as_str()).await.map_err(|e| transfer("resolve", &lexical, e))?
    } else {
        let (parent, name) = lexical.rsplit_once('/').unwrap_or(("", &lexical));
        let parent = if parent.is_empty() { "/" } else { parent };
        let real_parent = sftp.canonicalize(parent).await.map_err(|e| transfer("resolve", parent, e))?;
        format!("{}/{name}", real_parent.trim_end_matches('/'))
    };
    policy.check_path(&real, access)
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

#[cfg(unix)]
fn local_mode(meta: &std::fs::Metadata) -> u32 {
    std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777
}

#[cfg(not(unix))]
fn local_mode(_: &std::fs::Metadata) -> u32 {
    0o644
}

async fn hash_remote(sftp: &SftpSession, path: &str) -> Result<(u64, String), Error> {
    let mut file = sftp.open(path).await.map_err(|e| transfer("verify", path, e))?;
    let (mut hasher, mut total, mut buf) = (Sha256::new(), 0u64, vec![0u8; CHUNK]);
    loop {
        let n = file.read(&mut buf).await.map_err(|e| transfer("verify", path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((total, hex(&hasher.finalize())))
}

async fn upload(sftp: &SftpSession, local: &Path, temp: &str, mode: u32) -> Result<(u64, String), Error> {
    let mut src = tokio::fs::File::open(local).await.map_err(|e| transfer("read", local.display(), e.kind()))?;
    let attrs = FileAttributes { permissions: Some(mode), ..FileAttributes::default() };
    let mut dst = sftp
        .open_with_flags_and_attributes(temp, OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE, attrs)
        .await
        .map_err(|e| transfer("create", temp, e))?;
    let (mut hasher, mut total, mut buf) = (Sha256::new(), 0u64, vec![0u8; CHUNK]);
    loop {
        let n = src.read(&mut buf).await.map_err(|e| transfer("read", local.display(), e.kind()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        dst.write_all(&buf[..n]).await.map_err(|e| transfer("write", temp, e))?;
        total += n as u64;
    }
    // The server applies its umask on create; chmod only when the result differs.
    let granted = dst.metadata().await.map_err(|e| transfer("stat", temp, e))?.permissions.map(|p| p & 0o777);
    if granted != Some(mode) {
        let attrs = FileAttributes { permissions: Some(mode), ..FileAttributes::default() };
        dst.set_metadata(attrs).await.map_err(|e| transfer("chmod", temp, e))?;
    }
    dst.close().await.map_err(|e| transfer("close", temp, e))?;
    Ok((total, hex(&hasher.finalize())))
}

async fn publish(sftp: &SftpSession, temp: &str, target: &str, warnings: &mut Vec<String>) -> Result<(), Error> {
    let first = match sftp.rename(temp, target).await {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    if !sftp.try_exists(target).await.map_err(|e| transfer("stat", target, e))? {
        return Err(transfer("rename", target, first));
    }
    let backup = format!("{target}.sshhh-{}.old", token());
    sftp.rename(target, &backup).await.map_err(|e| transfer("rename", target, e))?;
    if let Err(e) = sftp.rename(temp, target).await {
        return Err(match sftp.rename(&backup, target).await {
            Ok(()) => transfer("rename", target, e),
            Err(restore) => transfer("rename", target, format!("{e}; the previous file is preserved as {backup} ({restore})")),
        });
    }
    if let Err(e) = sftp.remove_file(&backup).await {
        warnings.push(format!("replaced {target} but could not remove the previous copy {backup}: {e}"));
    }
    Ok(())
}

async fn discard_remote(sftp: &SftpSession, temp: &str, error: Error) -> Error {
    match sftp.remove_file(temp).await {
        Ok(()) => error,
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => error,
        Err(e) => transfer("cleanup", temp, format!("{error}; the temporary file was left behind ({e})")),
    }
}

pub async fn put(sftp: &SftpSession, policy: &Policy, local: &Path, remote: &str, opts: Options) -> Result<TransferReport, Error> {
    let started = Instant::now();
    let meta = tokio::fs::metadata(local).await.map_err(|e| transfer("read", local.display(), e.kind()))?;
    if !meta.is_file() {
        return Err(Error::Usage(format!("{} is not a regular file; directory transfer is not supported yet", local.display())));
    }
    let name = local.file_name().and_then(|n| n.to_str()).ok_or_else(|| Error::Usage(format!("{} has no usable file name", local.display())))?;

    let mut target = expand(sftp, remote).await?;
    let into_dir = target.ends_with('/') || remote_is_dir(sftp, &target).await? == Some(true);
    if into_dir {
        target = format!("{}/{name}", target.trim_end_matches('/'));
    }
    if remote_is_dir(sftp, &target).await? == Some(true) {
        return Err(transfer("put", &target, "is a directory"));
    }
    guard(sftp, policy, &target, Access::Write).await?;

    let mode = if opts.private { 0o600 } else { local_mode(&meta) };
    let temp = format!("{target}.sshhh-{}.part", token());
    let mut warnings = Vec::new();
    let (bytes, sha256) = match upload(sftp, local, &temp, mode).await {
        Ok(done) => done,
        Err(e) => return Err(discard_remote(sftp, &temp, e).await),
    };
    if opts.verify {
        let checked = match hash_remote(sftp, &temp).await {
            Ok(checked) => checked,
            Err(e) => return Err(discard_remote(sftp, &temp, e).await),
        };
        if checked != (bytes, sha256.clone()) {
            let error = Error::VerifyFailed { path: target.clone(), expected: format!("{bytes} bytes, sha256 {sha256}"), actual: format!("{} bytes, sha256 {}", checked.0, checked.1) };
            return Err(discard_remote(sftp, &temp, error).await);
        }
    }
    if let Err(e) = publish(sftp, &temp, &target, &mut warnings).await {
        return Err(discard_remote(sftp, &temp, e).await);
    }
    Ok(TransferReport {
        op: "put",
        local: local.display().to_string(),
        remote: target,
        bytes,
        sha256,
        verified: opts.verify,
        duration_ms: duration_ms(started),
        warnings,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListEntry {
    pub name: String,
    pub kind: &'static str,
    pub size: Option<u64>,
    pub mode: Option<String>,
    pub modified: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listing {
    pub path: String,
    pub entries: Vec<ListEntry>,
    pub hidden_by_policy: usize,
    pub warnings: Vec<String>,
}

pub async fn ls(sftp: &SftpSession, policy: &Policy, remote: &str) -> Result<Listing, Error> {
    let path = expand(sftp, remote).await?;
    guard(sftp, policy, &path, Access::Read).await?;
    let meta = sftp.metadata(path.as_str()).await.map_err(|e| transfer("stat", &path, e))?;
    if !meta.file_type().is_dir() {
        return Err(transfer("ls", &path, "is not a directory"));
    }
    let base = path.trim_end_matches('/');
    let absolute = base.starts_with('/');
    let (mut entries, mut hidden) = (Vec::new(), 0);
    for entry in sftp.read_dir(path.as_str()).await.map_err(|e| transfer("ls", &path, e))? {
        let name = entry.file_name();
        if absolute && policy.check_path(&format!("{base}/{name}"), Access::Read).is_err() {
            hidden += 1;
            continue;
        }
        let meta = entry.metadata();
        let kind = match entry.file_type() {
            t if t.is_dir() => "dir",
            t if t.is_symlink() => "symlink",
            t if t.is_file() => "file",
            _ => "other",
        };
        entries.push(ListEntry {
            name,
            kind,
            size: meta.size,
            mode: meta.permissions.map(|p| format!("{:04o}", p & 0o7777)),
            modified: meta.mtime,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Listing { path, entries, hidden_by_policy: hidden, warnings: Vec::new() })
}

pub async fn cat(sftp: &SftpSession, policy: &Policy, remote: &str, out: &mut (dyn FnMut(&[u8]) + Send)) -> Result<TransferReport, Error> {
    let started = Instant::now();
    let path = expand(sftp, remote).await?;
    guard(sftp, policy, &path, Access::Read).await?;
    let mut file = sftp.open(path.as_str()).await.map_err(|e| transfer("open", &path, e))?;
    let (mut hasher, mut total, mut buf) = (Sha256::new(), 0u64, vec![0u8; CHUNK]);
    loop {
        let n = file.read(&mut buf).await.map_err(|e| transfer("read", &path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out(&buf[..n]);
        total += n as u64;
    }
    Ok(TransferReport {
        op: "cat",
        local: String::new(),
        remote: path,
        bytes: total,
        sha256: hex(&hasher.finalize()),
        verified: false,
        duration_ms: duration_ms(started),
        warnings: Vec::new(),
    })
}

#[cfg(unix)]
async fn create_local(path: &Path, private: bool) -> std::io::Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    if private {
        options.mode(0o600);
    }
    options.open(path).await
}

#[cfg(not(unix))]
async fn create_local(path: &Path, private: bool) -> std::io::Result<tokio::fs::File> {
    let file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(path).await?;
    if private {
        restrict_to_owner(path).await?;
    }
    Ok(file)
}

#[cfg(not(unix))]
async fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    let var = |name: &str| std::env::var(name).map_err(|_| std::io::Error::other(format!("{name} is not set; cannot restrict the file")));
    let account = format!("{}\\{}", var("USERDOMAIN")?, var("USERNAME")?);
    let status = tokio::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("{account}:F"))
        .stdout(std::process::Stdio::null())
        .status()
        .await?;
    if status.success() { Ok(()) } else { Err(std::io::Error::other(format!("icacls exited with {status}"))) }
}

async fn download(sftp: &SftpSession, remote: &str, temp: &Path, expected: Option<u64>, private: bool) -> Result<(u64, String), Error> {
    let mut src = sftp.open(remote).await.map_err(|e| transfer("open", remote, e))?;
    let mut dst = create_local(temp, private).await.map_err(|e| transfer("create", temp.display(), e))?;
    let (mut hasher, mut total, mut buf) = (Sha256::new(), 0u64, vec![0u8; CHUNK]);
    loop {
        let n = src.read(&mut buf).await.map_err(|e| transfer("read", remote, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        dst.write_all(&buf[..n]).await.map_err(|e| transfer("write", temp.display(), e.kind()))?;
        total += n as u64;
    }
    dst.flush().await.map_err(|e| transfer("write", temp.display(), e.kind()))?;
    if let Some(size) = expected.filter(|size| *size != total) {
        return Err(transfer("read", remote, format!("received {total} bytes but the server reported {size}")));
    }
    Ok((total, hex(&hasher.finalize())))
}

async fn discard_local(temp: &Path, error: Error) -> Error {
    match tokio::fs::remove_file(temp).await {
        Ok(()) => error,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => error,
        Err(e) => transfer("cleanup", temp.display(), format!("{error}; the temporary file was left behind ({})", e.kind())),
    }
}

pub async fn get(sftp: &SftpSession, policy: &Policy, remote: &str, local: &Path, opts: Options) -> Result<TransferReport, Error> {
    let started = Instant::now();
    let remote = expand(sftp, remote).await?;
    guard(sftp, policy, &remote, Access::Read).await?;
    let meta = sftp.metadata(remote.as_str()).await.map_err(|e| transfer("stat", &remote, e))?;
    if !meta.file_type().is_file() {
        return Err(transfer("get", &remote, "is not a regular file; directory transfer is not supported yet"));
    }

    let wants_dir = local.as_os_str().to_str().is_some_and(|s| s.ends_with('/') || s.ends_with('\\'));
    let target: PathBuf = if wants_dir || tokio::fs::metadata(local).await.is_ok_and(|m| m.is_dir()) {
        local.join(basename(&remote))
    } else {
        local.to_path_buf()
    };
    if tokio::fs::metadata(&target).await.is_ok_and(|m| m.is_dir()) {
        return Err(transfer("get", target.display(), "is a directory"));
    }

    let mut temp_name = target.as_os_str().to_owned();
    temp_name.push(format!(".sshhh-{}.part", token()));
    let temp = PathBuf::from(temp_name);

    let (bytes, sha256) = match download(sftp, &remote, &temp, meta.size, opts.private).await {
        Ok(done) => done,
        Err(e) => return Err(discard_local(&temp, e).await),
    };
    if opts.verify {
        let checked = match hash_remote(sftp, &remote).await {
            Ok(checked) => checked,
            Err(e) => return Err(discard_local(&temp, e).await),
        };
        if checked != (bytes, sha256.clone()) {
            let error = Error::VerifyFailed { path: remote.clone(), expected: format!("{bytes} bytes, sha256 {sha256}"), actual: format!("{} bytes, sha256 {}", checked.0, checked.1) };
            return Err(discard_local(&temp, error).await);
        }
    }
    #[cfg(unix)]
    if !opts.private && let Some(mode) = meta.permissions {
        let perms = std::os::unix::fs::PermissionsExt::from_mode(mode & 0o777);
        if let Err(e) = tokio::fs::set_permissions(&temp, perms).await {
            return Err(discard_local(&temp, transfer("chmod", temp.display(), e.kind())).await);
        }
    }
    if let Err(e) = tokio::fs::rename(&temp, &target).await {
        return Err(discard_local(&temp, transfer("rename", target.display(), e.kind())).await);
    }
    Ok(TransferReport {
        op: "get",
        local: target.display().to_string(),
        remote,
        bytes,
        sha256,
        verified: opts.verify,
        duration_ms: duration_ms(started),
        warnings: Vec::new(),
    })
}
