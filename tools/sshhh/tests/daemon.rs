mod common;

use std::time::Duration;

use common::*;
use sshhh::daemon::{self, Client, Paths};
use sshhh::engine::{RunReport, RunRequest};
use sshhh::error::Error;
use sshhh::ops::Op;
use sshhh::session::Stream;
use sshhh::spec::{Context, ServerSpec};
use sshhh::transfer::Options;

struct Env {
    f: Fixture,
    home: tempfile::TempDir,
    client: Client,
    server: tokio::task::JoinHandle<Result<(), Error>>,
}

impl Env {
    async fn new() -> Env {
        let f = start(Behaviour::default()).await;
        let home = tempfile::tempdir().unwrap();
        let path = home.path().to_path_buf();
        let server = tokio::spawn(async move { daemon::serve(&path, Duration::from_secs(600)).await });
        let client = Client::new(home.path(), std::path::PathBuf::from("unused-in-tests"));
        let env = Env { f, home, client, server };
        env.wait_until_up().await;
        env
    }

    async fn wait_until_up(&self) {
        for _ in 0..100 {
            if matches!(self.client.status().await, Ok(Some(_))) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the daemon did not come up");
    }

    fn ctx(&self) -> Context {
        self.f.context(self.home.path())
    }

    async fn run(&self, spec: &ServerSpec, command: &str, stdin: Option<&[u8]>) -> Result<(RunReport, Vec<u8>, Vec<u8>), Error> {
        let op = Op::Run(RunRequest { command: command.into(), root: None, timeout_secs: 10 });
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let value = self
            .client
            .exec(&self.ctx(), spec, &op, stdin, &mut |stream, bytes| match stream {
                Stream::Stdout => out.extend_from_slice(bytes),
                Stream::Stderr => err.extend_from_slice(bytes),
            })
            .await?;
        Ok((serde_json::from_value(value).unwrap(), out, err))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn commands_stream_through_the_daemon_and_share_one_session() {
    let e = Env::new().await;
    let spec = e.f.spec();
    let (report, out, _) = e.run(&spec, "echo hi", None).await.unwrap();
    assert_eq!((report.rc, out.as_slice()), (0, b"hi\n".as_slice()));
    let (report, out, err) = e.run(&spec, "both", None).await.unwrap();
    assert_eq!((report.rc, out.as_slice(), err.as_slice()), (7, b"out\n".as_slice(), b"err\n".as_slice()));
    let status = e.client.status().await.unwrap().unwrap();
    assert_eq!(status.sessions.len(), 1);
    assert_eq!(status.sessions[0].alias, "test");
    assert!(!status.bw_session);
}

#[tokio::test(flavor = "multi_thread")]
async fn large_output_and_stdin_arrive_intact() {
    let e = Env::new().await;
    let spec = e.f.spec();
    let (report, out, _) = e.run(&spec, "bigout", None).await.unwrap();
    assert_eq!((report.rc, out.len()), (0, 64 * 16 * 1024));
    let input = vec![b'z'; 300_000];
    let (_, out, _) = e.run(&spec, "cat", Some(&input)).await.unwrap();
    assert_eq!(out, input);
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_keep_their_code_across_the_socket() {
    let e = Env::new().await;
    let mut spec = e.f.spec();
    spec.policy.add_layer("READONLY", "true").unwrap();
    let err = e.run(&spec, "rm -rf /", None).await.unwrap_err();
    assert_eq!(err.code(), "policy_denied");
    let mut wrong = e.f.spec();
    wrong.pass = Some("not the password".into());
    assert_eq!(e.run(&wrong, "echo x", None).await.unwrap_err().code(), "auth_failed");
}

#[tokio::test(flavor = "multi_thread")]
async fn files_move_through_the_daemon() {
    let e = Env::new().await;
    let spec = e.f.spec();
    let work = tempfile::tempdir().unwrap();
    let local = work.path().join("a.txt");
    std::fs::write(&local, b"payload").unwrap();
    let opts = Options { verify: true, private: false };
    let put = Op::Put { local: local.clone(), remote: "/home/deploy/a.txt".into(), opts };
    e.client.exec(&e.ctx(), &spec, &put, None, &mut |_: Stream, _: &[u8]| {}).await.unwrap();
    let back = work.path().join("b.txt");
    let get = Op::Get { remote: "/home/deploy/a.txt".into(), local: back.clone(), opts };
    e.client.exec(&e.ctx(), &spec, &get, None, &mut |_: Stream, _: &[u8]| {}).await.unwrap();
    assert_eq!(std::fs::read(&back).unwrap(), b"payload");
    let ls = e.client.exec(&e.ctx(), &spec, &Op::Ls { remote: "/home/deploy".into() }, None, &mut |_: Stream, _: &[u8]| {}).await.unwrap();
    assert!(ls.to_string().contains("a.txt"), "{ls}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_token_is_refused_and_the_daemon_keeps_serving() {
    let e = Env::new().await;
    let token = Paths::new(e.home.path()).token_path().to_path_buf();
    let good = std::fs::read_to_string(&token).unwrap();
    std::fs::write(&token, "0".repeat(good.len())).unwrap();
    assert_eq!(e.client.status().await.unwrap_err().code(), "refused");
    std::fs::write(&token, good).unwrap();
    assert!(e.client.status().await.unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_daemon_for_the_same_user_exits_quietly() {
    let e = Env::new().await;
    daemon::serve(e.home.path(), Duration::from_secs(600)).await.unwrap();
    assert!(e.client.status().await.unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_ends_the_daemon_and_removes_the_token() {
    let e = Env::new().await;
    assert!(e.client.stop().await.unwrap());
    tokio::time::timeout(Duration::from_secs(10), e.server).await.expect("the daemon did not stop").unwrap().unwrap();
    assert!(!Paths::new(e.home.path()).token_path().exists());
    assert!(e.client.status().await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_stored_bw_session_is_handed_to_bw_for_later_calls() {
    let e = Env::new().await;
    let dir = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let name = format!("bw_stub{}", std::env::consts::EXE_SUFFIX);
    let stub = exe.parent().unwrap().parent().unwrap().join("examples").join(&name);
    assert!(stub.is_file(), "build the example first: {}", stub.display());
    let bin = dir.path().join(format!("bw{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(&stub, &bin).unwrap();
    std::fs::create_dir(dir.path().join("data")).unwrap();
    std::fs::write(dir.path().join("data").join("locked"), "").unwrap();
    let item = serde_json::json!({ "name": "box", "login": { "username": USER, "password": PASSWORD } });
    std::fs::write(dir.path().join("data").join("item-box.json"), item.to_string()).unwrap();

    let mut ctx = e.ctx();
    ctx.settings.bw_bin = Some(bin.display().to_string());
    let mut spec = e.f.spec();
    spec.pass = Some("bw://box/password".into());
    let op = Op::Run(RunRequest { command: "echo hi".into(), root: None, timeout_secs: 10 });
    let mut sink = |_: Stream, _: &[u8]| {};

    let locked = e.client.exec(&ctx, &spec, &op, None, &mut sink).await.unwrap_err();
    assert_eq!(locked.code(), "secret_unavailable");
    assert!(locked.to_string().contains("vault_locked"), "{locked}");

    e.client.set_bw_session(&secrecy::SecretString::from("good-session")).await.unwrap();
    assert!(e.client.status().await.unwrap().unwrap().bw_session);
    e.client.exec(&ctx, &spec, &op, None, &mut sink).await.unwrap();
}
