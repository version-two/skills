use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use russh_sftp::protocol::{Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode};
use russh_sftp::server::Handler;

pub const HOME: &str = "/home/deploy";
const UMASK: u32 = 0o027;

/// A filesystem-backed SFTP server rooted in a temp directory. Remote paths are absolute inside
/// that root; `links` are virtual symlinks (from, to) applied to every path.
#[derive(Clone)]
pub struct SftpFs {
    pub root: PathBuf,
    pub links: Arc<Mutex<Vec<(String, String)>>>,
    pub modes: Arc<Mutex<HashMap<PathBuf, u32>>>,
    pub corrupt_part_reads: Arc<AtomicBool>,
    pub fail_writes_after: Arc<Mutex<Option<u64>>>,
}

impl SftpFs {
    pub fn new(root: PathBuf) -> SftpFs {
        std::fs::create_dir_all(root.join(HOME.trim_start_matches('/'))).unwrap();
        SftpFs { root, links: Default::default(), modes: Default::default(), corrupt_part_reads: Default::default(), fail_writes_after: Default::default() }
    }

    pub fn link(&self, from: &str, to: &str) {
        self.links.lock().unwrap().push((from.to_string(), to.to_string()));
    }

    pub fn real(&self, remote: &str) -> PathBuf {
        self.root.join(self.resolve(remote).trim_start_matches('/'))
    }

    pub fn mode(&self, remote: &str) -> Option<u32> {
        self.modes.lock().unwrap().get(&self.real(remote)).copied()
    }

    pub fn read(&self, remote: &str) -> Vec<u8> {
        std::fs::read(self.real(remote)).unwrap()
    }

    pub fn write(&self, remote: &str, bytes: &[u8]) {
        let path = self.real(remote);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    pub fn names(&self, remote: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.real(remote)).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    fn resolve(&self, path: &str) -> String {
        let absolute = if path.starts_with('/') { path.to_string() } else { format!("{HOME}/{path}") };
        let mut current = lexical(&absolute);
        let links = self.links.lock().unwrap();
        for _ in 0..8 {
            let hit = links.iter().find(|(from, _)| current == *from || current.starts_with(&format!("{from}/")));
            match hit {
                Some((from, to)) => current = lexical(&format!("{to}{}", &current[from.len()..])),
                None => break,
            }
        }
        current
    }

    fn attrs(&self, real: &Path) -> std::io::Result<FileAttributes> {
        let meta = std::fs::metadata(real)?;
        let stored = self.modes.lock().unwrap().get(real).copied();
        let (kind, mode) = if meta.is_dir() { (0x4000, stored.unwrap_or(0o755)) } else { (0x8000, stored.unwrap_or(0o644)) };
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as u32);
        Ok(FileAttributes { size: Some(meta.len()), permissions: Some(kind | mode), mtime, ..FileAttributes::default() })
    }
}

fn lexical(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    format!("/{}", out.join("/"))
}

enum Open {
    File { file: std::fs::File, path: PathBuf, written: u64 },
    Dir { entries: Option<Vec<File>> },
}

pub struct SftpHandler {
    fs: SftpFs,
    open: HashMap<String, Open>,
    next: u32,
}

impl SftpHandler {
    pub fn new(fs: SftpFs) -> SftpHandler {
        SftpHandler { fs, open: HashMap::new(), next: 0 }
    }

    fn handle(&mut self, open: Open) -> String {
        self.next += 1;
        let name = format!("h{}", self.next);
        self.open.insert(name.clone(), open);
        name
    }
}

fn ok(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() }
}

fn io_status(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

impl Handler for SftpHandler {
    type Error = StatusCode;

    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }

    async fn open(&mut self, id: u32, filename: String, pflags: OpenFlags, attrs: FileAttributes) -> Result<Handle, StatusCode> {
        let path = self.fs.real(&filename);
        let mut options = std::fs::OpenOptions::new();
        options
            .read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE))
            .append(pflags.contains(OpenFlags::APPEND))
            .truncate(pflags.contains(OpenFlags::TRUNCATE));
        if pflags.contains(OpenFlags::CREATE | OpenFlags::EXCLUDE) {
            options.create_new(true);
        } else if pflags.contains(OpenFlags::CREATE) {
            options.create(true);
        }
        let existed = path.exists();
        let file = options.open(&path).map_err(io_status)?;
        if !existed && let Some(mode) = attrs.permissions {
            self.fs.modes.lock().unwrap().insert(path.clone(), mode & 0o7777 & !UMASK);
        }
        Ok(Handle { id, handle: self.handle(Open::File { file, path, written: 0 }) })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusCode> {
        self.open.remove(&handle).map(|_| ok(id)).ok_or(StatusCode::Failure)
    }

    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, StatusCode> {
        let corrupt = self.fs.corrupt_part_reads.load(Ordering::Relaxed);
        let Some(Open::File { file, path, .. }) = self.open.get_mut(&handle) else { return Err(StatusCode::Failure) };
        file.seek(SeekFrom::Start(offset)).map_err(io_status)?;
        let mut buf = vec![0u8; len as usize];
        let n = file.read(&mut buf).map_err(io_status)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        buf.truncate(n);
        if corrupt && path.to_string_lossy().contains(".part") {
            buf[0] ^= 0xff;
        }
        Ok(Data { id, data: buf })
    }

    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, StatusCode> {
        let limit = *self.fs.fail_writes_after.lock().unwrap();
        let Some(Open::File { file, written, .. }) = self.open.get_mut(&handle) else { return Err(StatusCode::Failure) };
        if limit.is_some_and(|limit| *written + data.len() as u64 > limit) {
            return Err(StatusCode::Failure);
        }
        file.seek(SeekFrom::Start(offset)).map_err(io_status)?;
        file.write_all(&data).map_err(io_status)?;
        *written += data.len() as u64;
        Ok(ok(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        self.stat(id, path).await
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        let real = self.fs.real(&path);
        Ok(Attrs { id, attrs: self.fs.attrs(&real).map_err(io_status)? })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, StatusCode> {
        let Some(Open::File { path, .. }) = self.open.get(&handle) else { return Err(StatusCode::Failure) };
        Ok(Attrs { id, attrs: self.fs.attrs(path).map_err(io_status)? })
    }

    async fn setstat(&mut self, id: u32, path: String, attrs: FileAttributes) -> Result<Status, StatusCode> {
        let real = self.fs.real(&path);
        if !real.exists() {
            return Err(StatusCode::NoSuchFile);
        }
        if let Some(mode) = attrs.permissions {
            self.fs.modes.lock().unwrap().insert(real, mode & 0o7777);
        }
        Ok(ok(id))
    }

    async fn fsetstat(&mut self, id: u32, handle: String, attrs: FileAttributes) -> Result<Status, StatusCode> {
        let Some(Open::File { path, .. }) = self.open.get(&handle) else { return Err(StatusCode::Failure) };
        if let Some(mode) = attrs.permissions {
            self.fs.modes.lock().unwrap().insert(path.clone(), mode & 0o7777);
        }
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, StatusCode> {
        let real = self.fs.real(&path);
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&real).map_err(io_status)? {
            let entry = entry.map_err(io_status)?;
            let attrs = self.fs.attrs(&entry.path()).map_err(io_status)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            entries.push(File { filename: name.clone(), longname: name, attrs });
        }
        Ok(Handle { id, handle: self.handle(Open::Dir { entries: Some(entries) }) })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, StatusCode> {
        match self.open.get_mut(&handle) {
            Some(Open::Dir { entries }) => entries.take().map(|files| Name { id, files }).ok_or(StatusCode::Eof),
            _ => Err(StatusCode::Failure),
        }
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, StatusCode> {
        let real = self.fs.real(&filename);
        std::fs::remove_file(&real).map_err(io_status)?;
        self.fs.modes.lock().unwrap().remove(&real);
        Ok(ok(id))
    }

    async fn mkdir(&mut self, id: u32, path: String, _attrs: FileAttributes) -> Result<Status, StatusCode> {
        std::fs::create_dir(self.fs.real(&path)).map_err(io_status)?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, StatusCode> {
        std::fs::remove_dir(self.fs.real(&path)).map_err(io_status)?;
        Ok(ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, StatusCode> {
        Ok(Name { id, files: vec![File::dummy(self.fs.resolve(&path))] })
    }

    async fn rename(&mut self, id: u32, oldpath: String, newpath: String) -> Result<Status, StatusCode> {
        let (from, to) = (self.fs.real(&oldpath), self.fs.real(&newpath));
        if to.exists() {
            return Err(StatusCode::Failure);
        }
        std::fs::rename(&from, &to).map_err(io_status)?;
        let mut modes = self.fs.modes.lock().unwrap();
        if let Some(mode) = modes.remove(&from) {
            modes.insert(to, mode);
        }
        Ok(ok(id))
    }
}
