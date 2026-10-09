use std::io::Write;
use std::path::Path;

/// Removes every access except the owner's. On Windows this shells out to `icacls`; the result is
/// what `icacls` reports, not an independent check.
#[cfg(windows)]
pub fn restrict_to_owner(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("*{}:F", current_user_sid()?))
        .stdout(std::process::Stdio::null())
        .creation_flags(0x0800_0000)
        .status()?;
    if status.success() { Ok(()) } else { Err(std::io::Error::other(format!("icacls exited with {status}"))) }
}

/// The account SID of the running user, as `S-1-5-21-...`. It is what the file and pipe ACLs
/// name, so they hold for an elevated and a normal token of the same user alike.
#[cfg(windows)]
pub fn current_user_sid() -> std::io::Result<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: every pointer handed to the Win32 calls refers to a live local buffer of the stated
    // size, and every handle or allocation obtained here is released before returning.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let result = (|| {
            let mut needed = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
            let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
            if GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), needed, &mut needed) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut text: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut len = 0;
            while *text.add(len) != 0 {
                len += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            Ok(sid)
        })();
        CloseHandle(token);
        result
    }
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn the_sid_is_a_user_account_sid_and_files_end_up_with_only_that_grant() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        write_private(&path, b"x").unwrap();
        let out = std::process::Command::new("icacls").arg(&path).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
        assert!(!text.contains("everyone") && !text.contains("builtin\\users"), "{text}");
        assert_eq!(text.matches(":(f)").count(), 1, "{text}");
    }
}
