use std::io::Write;
use std::path::Path;

/// Removes every access except the owner's. On Windows this shells out to `icacls`; the result is
/// what `icacls` reports, not an independent check.
#[cfg(windows)]
pub fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let var = |name: &str| std::env::var(name).map_err(|_| std::io::Error::other(format!("{name} is not set; cannot restrict the file")));
    let account = format!("{}\\{}", var("USERDOMAIN")?, var("USERNAME")?);
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("{account}:F"))
        .stdout(std::process::Stdio::null())
        .creation_flags(0x0800_0000)
        .status()?;
    if status.success() { Ok(()) } else { Err(std::io::Error::other(format!("icacls exited with {status}"))) }
}

#[cfg(unix)]
pub fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// Creates `path` readable by the owner only and fails if it already exists.
pub fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(path)?;
    #[cfg(windows)]
    if let Err(e) = restrict_to_owner(path) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(file)
}

/// Atomically replaces `path` with `bytes`, private to the owner.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = std::path::PathBuf::from(temp);
    let _ = std::fs::remove_file(&temp);
    let mut file = create_private(&temp)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written.and_then(|()| std::fs::rename(&temp, path)) {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    Ok(())
}
