use std::path::{Path, PathBuf};
use std::time::Duration;

/// Copies the `bw` test double into `dir/bw` and returns its path.
///
/// On Linux a parallel test can fork while the copy is still open for writing, and the new
/// program then cannot be executed ("text file busy") until that child has exec'd. The stub
/// answers `--selftest` without side effects, so the wait ends the first time it starts.
pub fn install_bw_stub(dir: &Path) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let name = format!("bw_stub{}", std::env::consts::EXE_SUFFIX);
    let stub = exe.parent().unwrap().parent().unwrap().join("examples").join(name);
    assert!(stub.is_file(), "build the example first: {}", stub.display());
    let bin = dir.join(format!("bw{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(&stub, &bin).unwrap();
    for _ in 0..200 {
        match std::process::Command::new(&bin).arg("--selftest").status() {
            Ok(status) if status.success() => return bin,
            Ok(status) => panic!("the bw stub failed its selftest: {status}"),
            Err(e) if e.raw_os_error() == Some(26) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => panic!("cannot start the bw stub: {e}"),
        }
    }
    panic!("the bw stub stayed busy");
}
