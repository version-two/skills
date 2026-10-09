mod common;

use std::time::Duration;

use common::*;
use sshhh::audit;
use sshhh::engine::{Engine, RunReport, RunRequest};
use sshhh::error::Error;
use sshhh::escalate::RootMode;
use sshhh::session::Stream;
use sshhh::spec::{Context, ServerSpec};

struct Ran {
    report: RunReport,
    stdout: String,
    stderr: String,
}

async fn run(engine: &Engine, ctx: &Context, spec: &ServerSpec, command: &str, root: Option<RootMode>, stdin: Option<&[u8]>) -> Result<Ran, Error> {
    let request = RunRequest { command: command.into(), root, timeout_secs: 10 };
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let report = engine
        .run(ctx, spec, &request, stdin.map(<[u8]>::to_vec), &mut |stream, bytes| match stream {
            Stream::Stdout => stdout.extend_from_slice(bytes),
            Stream::Stderr => stderr.extend_from_slice(bytes),
        })
        .await?;
    Ok(Ran { report, stdout: String::from_utf8(stdout).unwrap(), stderr: String::from_utf8(stderr).unwrap() })
}

fn audit_lines(home: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(audit::path(home)).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[tokio::test]
async fn one_cached_session_serves_every_call_and_the_exit_status_passes_through() {
    let f = start(Behaviour::default()).await;
    let home = tempfile::tempdir().unwrap();
    let (engine, ctx, spec) = (Engine::new(), f.context(home.path()), f.spec());
    for i in 0..3 {
        let r = run(&engine, &ctx, &spec, &format!("echo n{i}"), None, None).await.unwrap();
        assert_eq!((r.report.rc, r.stdout.as_str(), r.report.escalation.as_str()), (0, format!("n{i}\n").as_str(), "none"));
    }
    assert_eq!(engine.session_count().await, 1);
    let r = run(&engine, &ctx, &spec, "both", None, None).await.unwrap();
    assert_eq!((r.report.rc, r.stdout.as_str(), r.stderr.as_str()), (7, "out\n", "err\n"));
    assert_eq!(engine.sweep(Duration::ZERO).await, 1);
    assert_eq!(engine.session_count().await, 0);
}

#[tokio::test]
async fn configured_secrets_are_masked_in_output() {
    let f = start(Behaviour::default()).await;
    let home = tempfile::tempdir().unwrap();
    let r = run(&Engine::new(), &f.context(home.path()), &f.spec(), &format!("echo the password is {PASSWORD}!"), None, None).await.unwrap();
    assert_eq!(r.stdout, "the password is ***!\n");
}

#[tokio::test]
async fn every_call_is_audited_without_the_command_text() {
    let f = start(Behaviour::default()).await;
    let home = tempfile::tempdir().unwrap();
    let (engine, ctx, spec) = (Engine::new(), f.context(home.path()), f.spec());
    run(&engine, &ctx, &spec, "echo secret-looking-command", None, None).await.unwrap();
    let _ = run(&engine, &ctx, &spec, "dropconn", None, None).await;
    let _ = run(&engine, &ctx, &spec, "exit 3", None, None).await.unwrap();
    let lines = audit_lines(home.path());
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1]["error"], "connection_lost");
    assert_eq!((lines[0]["alias"].as_str(), lines[0]["user"].as_str(), lines[0]["rc"].as_i64()), (Some("test"), Some(USER), Some(0)));
    assert_eq!(lines[2]["rc"], 3);
    assert!(lines.iter().all(|l| l.get("subject").is_none()));
    assert!(!std::fs::read_to_string(audit::path(home.path())).unwrap().contains("secret-looking-command"));

    let mut with_text = f.context(home.path());
    with_text.audit_commands = true;
    run(&engine, &with_text, &spec, "echo visible", None, None).await.unwrap();
    assert_eq!(audit_lines(home.path()).last().unwrap()["subject"], "echo visible");
}

#[tokio::test]
async fn an_unwritable_audit_log_stops_the_call_before_it_connects() {
    let f = start(Behaviour::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let not_a_dir = dir.path().join("file");
    std::fs::write(&not_a_dir, "x").unwrap();
    let err = run(&Engine::new(), &f.context(&not_a_dir), &f.spec(), "echo hi", None, None).await.err().unwrap();
    assert_eq!(err.code(), "io_error");
    assert!(!f.known_hosts.exists(), "no connection should have been attempted");
}

#[tokio::test]
async fn sudo_sends_the_password_on_stdin_and_forwards_the_rest() {
    let f = start(Behaviour::default()).await;
    let home = tempfile::tempdir().unwrap();
    let (engine, ctx, spec) = (Engine::new(), f.context(home.path()), f.spec());
    let r = run(&engine, &ctx, &spec, "systemctl restart x", Some(RootMode::Auto), None).await.unwrap();
    assert_eq!((r.report.rc, r.stdout.as_str(), r.report.escalation.as_str()), (0, "ran as root: systemctl restart x\n", "sudo"));
    let r = run(&engine, &ctx, &spec, "tee out", Some(RootMode::Sudo), Some(b"piped line\n")).await.unwrap();
    assert_eq!(r.stdout, "ran as root: tee out\npiped line\n");
    assert!(!audit_lines(home.path()).iter().any(|l| l.to_string().contains(SUDO_PASSWORD)));
}

#[tokio::test]
async fn a_rejected_sudo_password_is_an_escalation_failure() {
    let f = start(Behaviour::default()).await;
    let home = tempfile::tempdir().unwrap();
    let mut spec = f.spec();
    spec.sudo_pass = Some("wrong".into());
    let err = run(&Engine::new(), &f.context(home.path()), &spec, "id", Some(RootMode::Auto), None).await.err().unwrap();
    assert_eq!(err.code(), "escalation_failed");
    assert!(err.to_string().contains("rejected"), "{err}");
}

#[tokio::test]
async fn passwordless_sudo_never_sends_the_password() {
    let f = start(Behaviour { sudo_nopasswd: true, ..Default::default() }).await;
    let home = tempfile::tempdir().unwrap();
    let r = run(&Engine::new(), &f.context(home.path()), &f.spec(), "id", Some(RootMode::Auto), Some(b"stays with the user\n")).await.unwrap();
    assert_eq!(r.stdout, "ran as root: id\n");

    let mut spec = f.spec();
    spec.sudo_pass = None;
    let r = run(&Engine::new(), &f.context(home.path()), &spec, "id", Some(RootMode::Auto), None).await.unwrap();
    assert_eq!(r.report.escalation, "sudo");
}

#[tokio::test]
async fn sudo_without_a_password_says_what_to_set() {
    let f = start(Behaviour::default()).await;
    let home = tempfile::tempdir().unwrap();
    let mut spec = f.spec();
    spec.sudo_pass = None;
    let err = run(&Engine::new(), &f.context(home.path()), &spec, "id", Some(RootMode::Auto), None).await.err().unwrap();
    assert_eq!(err.code(), "escalation_failed");
    assert!(err.to_string().contains("SUDO_PASS"), "{err}");
}

fn su_spec(f: &Fixture, password: &str) -> ServerSpec {
    let mut spec = f.spec();
    spec.sudo_pass = None;
    spec.root_pass = Some(password.into());
    spec
}

#[tokio::test]
async fn su_answers_the_prompt_on_a_pty_and_cleans_the_line_endings() {
    let f = start(Behaviour::default()).await;
    let home = tempfile::tempdir().unwrap();
    let r = run(&Engine::new(), &f.context(home.path()), &su_spec(&f, ROOT_PASSWORD), "echo hi", Some(RootMode::Auto), None).await.unwrap();
    assert_eq!((r.report.rc, r.stdout.as_str(), r.report.escalation.as_str()), (0, "ran as root: echo hi\n", "su"));
    assert!(r.stderr.is_empty());
}

#[tokio::test]
async fn a_rejected_root_password_is_an_escalation_failure_and_never_echoed() {
    let f = start(Behaviour::default()).await;
    let home = tempfile::tempdir().unwrap();
    let err = run(&Engine::new(), &f.context(home.path()), &su_spec(&f, "nope-nope"), "id", Some(RootMode::Su), None).await.err().unwrap();
    assert_eq!(err.code(), "escalation_failed");
    assert!(!err.to_string().contains("nope-nope"));
}

#[tokio::test]
async fn su_fails_fast_when_no_prompt_appears() {
    let f = start(Behaviour { su_prompts: false, ..Default::default() }).await;
    let home = tempfile::tempdir().unwrap();
    let engine = Engine::new().with_prompt_timeout(Duration::from_millis(300));
    let err = run(&engine, &f.context(home.path()), &su_spec(&f, ROOT_PASSWORD), "id", Some(RootMode::Su), None).await.err().unwrap();
    assert_eq!(err.code(), "escalation_failed");
    assert!(err.to_string().contains("no password prompt"), "{err}");
}

#[tokio::test]
async fn su_refuses_standard_input() {
    let f = start(Behaviour::default()).await;
    let home = tempfile::tempdir().unwrap();
    let err = run(&Engine::new(), &f.context(home.path()), &su_spec(&f, ROOT_PASSWORD), "id", Some(RootMode::Su), Some(b"data")).await.err().unwrap();
    assert_eq!(err.code(), "usage");
}

#[tokio::test]
async fn a_connection_lost_mid_command_is_reported_never_re_run() {
    let f = start(Behaviour::default()).await;
    let home = tempfile::tempdir().unwrap();
    let (engine, ctx, spec) = (Engine::new(), f.context(home.path()), f.spec());
    let err = run(&engine, &ctx, &spec, "dropconn", None, None).await.err().unwrap();
    assert_eq!(err.code(), "connection_lost");
    assert_eq!(audit_lines(home.path()).len(), 1, "one attempt, no silent retry");
    let r = run(&engine, &ctx, &spec, "echo again", None, None).await.unwrap();
    assert_eq!(r.stdout, "again\n");
}

#[tokio::test]
async fn a_session_that_died_between_calls_is_replaced_before_the_next_command() {
    let f = start(Behaviour::default()).await;
    let home = tempfile::tempdir().unwrap();
    let (engine, ctx, spec) = (Engine::new(), f.context(home.path()), f.spec());
    run(&engine, &ctx, &spec, "dropsoon", None, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r = run(&engine, &ctx, &spec, "echo back", None, None).await.unwrap();
    assert_eq!((r.stdout.as_str(), r.report.reconnected), ("back\n", true));
}
