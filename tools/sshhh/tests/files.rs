mod common;

use std::path::Path;
use std::sync::atomic::Ordering;

use common::*;
use sshhh::audit;
use sshhh::engine::Engine;
use sshhh::error::Error;
use sshhh::session::Stream;
use sshhh::spec::ServerSpec;
use sshhh::transfer::Options;

const VERIFY: Options = Options { verify: true, private: false };

struct Env {
    f: Fixture,
    home: tempfile::TempDir,
    work: tempfile::TempDir,
    engine: Engine,
}

impl Env {
    async fn new() -> Env {
        Env { f: start(Behaviour::default()).await, home: tempfile::tempdir().unwrap(), work: tempfile::tempdir().unwrap(), engine: Engine::new() }
    }

    fn spec(&self, policy: &[(&str, &str)]) -> ServerSpec {
        let mut spec = self.f.spec();
        for (key, value) in policy {
            spec.policy.add_layer(key, value).unwrap();
        }
        spec
    }

    fn local(&self, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = self.work.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    async fn put(&self, spec: &ServerSpec, local: &Path, remote: &str, opts: Options) -> Result<sshhh::transfer::TransferReport, Error> {
        self.engine.put(&self.f.context(self.home.path()), spec, local, remote, opts).await
    }

    async fn get(&self, spec: &ServerSpec, remote: &str, local: &Path, opts: Options) -> Result<sshhh::transfer::TransferReport, Error> {
        self.engine.get(&self.f.context(self.home.path()), spec, remote, local, opts).await
    }

    async fn cat(&self, spec: &ServerSpec, remote: &str) -> Result<String, Error> {
        let mut out = Vec::new();
        self.engine
            .cat(&self.f.context(self.home.path()), spec, remote, &mut |_: Stream, bytes: &[u8]| out.extend_from_slice(bytes))
            .await?;
        Ok(String::from_utf8(out).unwrap())
    }

    fn audit(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(audit::path(self.home.path())).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
}

fn code<T: std::fmt::Debug>(result: Result<T, Error>) -> &'static str {
    result.unwrap_err().code()
}

#[tokio::test]
async fn put_and_get_round_trip_with_verification() {
    let e = Env::new().await;
    let spec = e.spec(&[]);
    let payload: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
    let local = e.local("data.bin", &payload);

    let up = e.put(&spec, &local, "/home/deploy/data.bin", VERIFY).await.unwrap();
    assert_eq!((up.bytes, up.verified, up.remote.as_str()), (payload.len() as u64, true, "/home/deploy/data.bin"));
    assert_eq!(e.f.fs.read("/home/deploy/data.bin"), payload);
    assert_eq!(e.f.fs.names("/home/deploy"), ["data.bin"]);

    let back = e.work.path().join("back.bin");
    let down = e.get(&spec, "~/data.bin", &back, VERIFY).await.unwrap();
    assert_eq!((std::fs::read(&back).unwrap(), down.sha256), (payload, up.sha256));
}

#[tokio::test]
async fn a_directory_target_receives_the_file_under_its_own_name() {
    let e = Env::new().await;
    let spec = e.spec(&[]);
    let local = e.local("report.txt", b"hello");
    e.f.fs.write("/home/deploy/inbox/.keep", b"");
    e.put(&spec, &local, "/home/deploy/inbox/", Options::default()).await.unwrap();
    e.put(&spec, &local, "inbox", Options::default()).await.unwrap();
    assert_eq!(e.f.fs.names("/home/deploy/inbox"), [".keep", "report.txt"]);

    let dest = e.work.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let got = e.get(&spec, "/home/deploy/inbox/report.txt", &dest, Options::default()).await.unwrap();
    assert_eq!(std::fs::read(dest.join("report.txt")).unwrap(), b"hello");
    assert!(got.local.ends_with("report.txt"));
}

#[tokio::test]
async fn overwriting_replaces_the_file_and_leaves_nothing_behind() {
    let e = Env::new().await;
    let spec = e.spec(&[]);
    e.f.fs.write("/home/deploy/conf", b"old");
    let report = e.put(&spec, &e.local("conf", b"new content"), "/home/deploy/conf", VERIFY).await.unwrap();
    assert_eq!(e.f.fs.read("/home/deploy/conf"), b"new content");
    assert_eq!(e.f.fs.names("/home/deploy"), ["conf"]);
    assert!(report.warnings.is_empty());
}

#[tokio::test]
async fn private_uploads_are_mode_0600_and_others_keep_the_local_mode() {
    let e = Env::new().await;
    let spec = e.spec(&[]);
    let local = e.local("key", b"secret");
    e.put(&spec, &local, "/home/deploy/key", Options { verify: false, private: true }).await.unwrap();
    assert_eq!(e.f.fs.mode("/home/deploy/key"), Some(0o600));
    e.put(&spec, &local, "/home/deploy/plain", Options::default()).await.unwrap();
    assert_eq!(e.f.fs.mode("/home/deploy/plain"), Some(0o644));
}

#[cfg(unix)]
#[tokio::test]
async fn downloads_are_private_on_request_and_keep_the_remote_mode_otherwise() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new().await;
    let spec = e.spec(&[]);
    e.f.fs.write("/home/deploy/x", b"1");
    let mode_of = |name: &str| std::fs::metadata(e.work.path().join(name)).unwrap().permissions().mode() & 0o777;
    e.get(&spec, "~/x", &e.work.path().join("private"), Options { verify: false, private: true }).await.unwrap();
    e.get(&spec, "~/x", &e.work.path().join("plain"), Options::default()).await.unwrap();
    assert_eq!((mode_of("private"), mode_of("plain")), (0o600, 0o644));
}

#[tokio::test]
async fn a_corrupted_upload_fails_verification_and_never_replaces_the_target() {
    let e = Env::new().await;
    let spec = e.spec(&[]);
    e.f.fs.write("/home/deploy/live", b"good");
    e.f.fs.corrupt_part_reads.store(true, Ordering::Relaxed);
    let err = e.put(&spec, &e.local("live", b"replacement"), "/home/deploy/live", VERIFY).await.unwrap_err();
    assert_eq!(err.code(), "verify_failed");
    assert_eq!(e.f.fs.read("/home/deploy/live"), b"good");
    assert_eq!(e.f.fs.names("/home/deploy"), ["live"]);
}

#[tokio::test]
async fn a_failed_write_removes_the_temporary_files_on_both_sides() {
    let e = Env::new().await;
    let spec = e.spec(&[]);
    *e.f.fs.fail_writes_after.lock().unwrap() = Some(10);
    assert_eq!(code(e.put(&spec, &e.local("big", &[7u8; 5000]), "/home/deploy/big", Options::default()).await), "transfer_failed");
    assert!(e.f.fs.names("/home/deploy").is_empty());

    *e.f.fs.fail_writes_after.lock().unwrap() = None;
    e.f.fs.write("/home/deploy/src", b"abc");
    assert_eq!(code(e.get(&spec, "/home/deploy/src", &e.work.path().join("a").join("b"), Options::default()).await), "transfer_failed");
    assert_eq!(code(e.get(&spec, "/home/deploy/missing", &e.work.path().join("m"), Options::default()).await), "transfer_failed");
    let leftovers: Vec<String> = std::fs::read_dir(e.work.path()).unwrap().map(|x| x.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".sshhh-")).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test]
async fn ls_and_cat_read_without_a_shell_and_mask_secrets() {
    let e = Env::new().await;
    let spec = e.spec(&[]);
    e.f.fs.write("/home/deploy/b.txt", format!("token={PASSWORD}\n").as_bytes());
    e.f.fs.write("/home/deploy/a/inner", b"x");
    let listing = e.engine.ls(&e.f.context(e.home.path()), &spec, "~").await.unwrap();
    let names: Vec<(&str, &str)> = listing.entries.iter().map(|x| (x.name.as_str(), x.kind)).collect();
    assert_eq!((names, listing.hidden_by_policy), (vec![("a", "dir"), ("b.txt", "file")], 0));
    assert_eq!(e.cat(&spec, "~/b.txt").await.unwrap(), "token=***\n");
    assert_eq!(code(e.cat(&spec, "~/a").await), "transfer_failed");
}

#[tokio::test]
async fn readonly_refuses_changes_before_connecting_and_still_reads() {
    let e = Env::new().await;
    let spec = e.spec(&[("READONLY", "true"), ("ALLOW_COMMANDS", "echo")]);
    e.f.fs.write("/home/deploy/notes", b"visible");
    let ctx = e.f.context(e.home.path());

    assert_eq!(code(e.put(&spec, &e.local("x", b"1"), "/home/deploy/x", Options::default()).await), "policy_denied");
    assert_eq!(e.engine.session_count().await, 0);
    assert!(!e.f.fs.real("/home/deploy/x").exists());

    assert_eq!(e.cat(&spec, "~/notes").await.unwrap(), "visible");
    e.get(&spec, "~/notes", &e.work.path().join("n"), Options::default()).await.unwrap();
    e.engine.ls(&ctx, &spec, "~").await.unwrap();

    let request = |command: &str, root| sshhh::engine::RunRequest { command: command.into(), root, timeout_secs: 10 };
    let mut sink = |_: Stream, _: &[u8]| {};
    e.engine.run(&ctx, &spec, &request("echo fine", None), None, &mut sink).await.unwrap();
    for (command, root) in [("rm -rf /", None), ("echo x > /tmp/f", None), ("echo x", Some(sshhh::escalate::RootMode::Sudo))] {
        let err = e.engine.run(&ctx, &spec, &request(command, root), None, &mut sink).await.unwrap_err();
        assert_eq!(err.code(), "policy_denied", "{command}");
    }

    let audited: Vec<(String, Option<String>)> =
        e.audit().iter().map(|l| (l["op"].as_str().unwrap().to_string(), l["error"].as_str().map(str::to_string))).collect();
    assert_eq!(audited[0], ("put".to_string(), Some("policy_denied".to_string())));
    assert_eq!(audited.last().unwrap(), &("run".to_string(), Some("policy_denied".to_string())));
}

#[tokio::test]
async fn a_server_without_a_policy_is_unrestricted() {
    let e = Env::new().await;
    let spec = e.spec(&[]);
    let ctx = e.f.context(e.home.path());
    let request = sshhh::engine::RunRequest { command: "echo $(anything) `goes`".into(), root: None, timeout_secs: 10 };
    e.engine.run(&ctx, &spec, &request, None, &mut |_: Stream, _: &[u8]| {}).await.unwrap();
    e.put(&spec, &e.local("f", b"1"), "/home/deploy/f", Options::default()).await.unwrap();
    assert_eq!(code(e.put(&spec, &e.local("g", b"1"), "/nonexistent/dir/g", Options::default()).await), "transfer_failed");
}

#[tokio::test]
async fn path_rules_hold_for_the_path_as_written_and_as_the_server_resolves_it() {
    let e = Env::new().await;
    let spec = e.spec(&[("ALLOW_PATHS", "/home/deploy/www"), ("DENY_PATHS", "**/.env")]);
    e.f.fs.write("/home/deploy/www/index.html", b"<html>");
    e.f.fs.write("/home/deploy/www/.env", b"APP_KEY=1");
    e.f.fs.write("/home/deploy/private.txt", b"nope");
    e.f.fs.write("/etc/passwd", b"root:x:0:0");
    e.f.fs.link("/home/deploy/www/etc", "/etc");
    let local = e.local("up", b"1");

    assert_eq!(e.cat(&spec, "/home/deploy/www/index.html").await.unwrap(), "<html>");
    e.put(&spec, &local, "/home/deploy/www/new.html", Options::default()).await.unwrap();
    e.put(&spec, &local, "www/relative.html", Options::default()).await.unwrap();

    for remote in ["/home/deploy/private.txt", "/home/deploy/www/.env", "/home/deploy/www/../private.txt", "/home/deploy/www/etc/passwd", "/home/deploy/wwwx/a"] {
        assert_eq!(code(e.cat(&spec, remote).await), "policy_denied", "{remote}");
    }
    for remote in ["/home/deploy/elsewhere.txt", "/home/deploy/www/etc/dropped", "/home/deploy/www/.env"] {
        assert_eq!(code(e.put(&spec, &local, remote, Options::default()).await), "policy_denied", "{remote}");
    }
    assert_eq!(code(e.get(&spec, "/home/deploy/www/etc/passwd", &e.work.path().join("p"), Options::default()).await), "policy_denied");
    assert!(!e.f.fs.real("/etc/dropped").exists());
    assert_eq!(e.f.fs.read("/home/deploy/www/.env"), b"APP_KEY=1");

    let listing = e.engine.ls(&e.f.context(e.home.path()), &spec, "/home/deploy/www").await.unwrap();
    let names: Vec<&str> = listing.entries.iter().map(|x| x.name.as_str()).collect();
    assert_eq!((names, listing.hidden_by_policy), (vec!["index.html", "new.html", "relative.html"], 1));
}

#[tokio::test]
async fn command_rules_apply_to_exec_before_anything_connects() {
    let e = Env::new().await;
    let spec = e.spec(&[("DENY_COMMANDS", "rm;dd"), ("ALLOW_COMMANDS", "echo;rm;exit")]);
    let ctx = e.f.context(e.home.path());
    let request = |command: &str| sshhh::engine::RunRequest { command: command.into(), root: None, timeout_secs: 10 };
    let mut sink = |_: Stream, _: &[u8]| {};
    e.engine.run(&ctx, &spec, &request("echo ok"), None, &mut sink).await.unwrap();
    for command in ["rm x", "sh -c 'rm x'", "echo a; ls", "echo $(id)"] {
        let err = e.engine.run(&ctx, &spec, &request(command), None, &mut sink).await.unwrap_err();
        assert_eq!(err.code(), "policy_denied", "{command}");
    }
}

#[tokio::test]
async fn a_later_layer_cannot_lift_a_restriction() {
    let e = Env::new().await;
    let mut spec = e.spec(&[("READONLY", "true")]);
    spec.policy.add_layer("READONLY", "false").unwrap();
    assert_eq!(code(e.put(&spec, &e.local("x", b"1"), "/home/deploy/x", Options::default()).await), "policy_denied");
}
