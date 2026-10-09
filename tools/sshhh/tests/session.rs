mod common;

use std::time::Duration;

use common::*;
use secrecy::SecretString;
use sshhh::config::AuthMethod;
use sshhh::session::{HostKeyPolicy, Session, Stream};

async fn run(session: &Session, command: &str, stdin: Option<&[u8]>) -> (sshhh::session::ExecOutcome, Vec<u8>, Vec<u8>) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let outcome = session
        .exec(command, stdin.map(<[u8]>::to_vec), Duration::from_secs(10), &mut |stream, bytes| match stream {
            Stream::Stdout => out.extend_from_slice(bytes),
            Stream::Stderr => err.extend_from_slice(bytes),
        })
        .await
        .unwrap();
    (outcome, out, err)
}

async fn connected(f: &Fixture) -> Session {
    Session::connect(&f.creds(), &f.opts(HostKeyPolicy::AcceptNew)).await.unwrap()
}

#[tokio::test]
async fn password_auth_runs_a_command() {
    let f = start(Behaviour::default()).await;
    let s = connected(&f).await;
    let (o, out, err) = run(&s, "echo hello world", None).await;
    assert_eq!((o.rc, o.signal), (0, None));
    assert_eq!(out, b"hello world\n");
    assert!(err.is_empty());
}

#[tokio::test]
async fn remote_exit_status_passes_through_and_streams_stay_separate() {
    let f = start(Behaviour::default()).await;
    let s = connected(&f).await;
    assert_eq!(run(&s, "exit 3", None).await.0.rc, 3);
    let (o, out, err) = run(&s, "both", None).await;
    assert_eq!((o.rc, out.as_slice(), err.as_slice()), (7, &b"out\n"[..], &b"err\n"[..]));
}

#[tokio::test]
async fn one_session_serves_many_commands() {
    let f = start(Behaviour::default()).await;
    let s = connected(&f).await;
    for i in 0..20 {
        let (o, out, _) = run(&s, &format!("echo n{i}"), None).await;
        assert_eq!(o.rc, 0);
        assert_eq!(out, format!("n{i}\n").into_bytes());
    }
}

#[tokio::test]
async fn stdin_reaches_the_command_and_is_closed() {
    let f = start(Behaviour::default()).await;
    let s = connected(&f).await;
    let (o, out, _) = run(&s, "cat", Some(b"piped in\nsecond line\n")).await;
    assert_eq!((o.rc, out.as_slice()), (0, &b"piped in\nsecond line\n"[..]));
    let (o, out, _) = run(&s, "cat", None).await;
    assert_eq!((o.rc, out.len()), (0, 0));
}

#[tokio::test]
async fn large_output_arrives_complete() {
    let f = start(Behaviour::default()).await;
    let s = connected(&f).await;
    let (o, out, _) = run(&s, "bigout", None).await;
    assert_eq!((o.rc, out.len()), (0, 64 * 16 * 1024));
}

#[tokio::test]
async fn killed_by_signal_reports_the_signal() {
    let f = start(Behaviour::default()).await;
    let s = connected(&f).await;
    let (o, _, _) = run(&s, "kill", None).await;
    assert_eq!((o.rc, o.signal.as_deref()), (137, Some("KILL")));
}

#[tokio::test]
async fn missing_exit_status_is_connection_lost_not_success() {
    let f = start(Behaviour::default()).await;
    let s = connected(&f).await;
    let err = s.exec("noexit", None, Duration::from_secs(10), &mut |_, _| {}).await.unwrap_err();
    assert_eq!(err.code(), "connection_lost");
}

#[tokio::test]
async fn command_timeout_is_an_error() {
    let f = start(Behaviour::default()).await;
    let s = connected(&f).await;
    let err = s.exec("sleep", None, Duration::from_millis(300), &mut |_, _| {}).await.unwrap_err();
    assert_eq!(err.code(), "command_timeout");
}

#[tokio::test]
async fn wrong_password_fails_with_tried_methods() {
    let f = start(Behaviour::default()).await;
    let mut creds = f.creds();
    creds.pass = Some(SecretString::from("nope"));
    let err = Session::connect(&creds, &f.opts(HostKeyPolicy::AcceptNew)).await.err().unwrap();
    assert_eq!(err.code(), "auth_failed");
    assert!(err.to_string().contains("password"));
    assert!(!err.to_string().contains("nope"));
}

#[tokio::test]
async fn keyboard_interactive_answers_with_the_password() {
    let f = start(Behaviour { allow_password: false, kbdint: true, ..Default::default() }).await;
    let mut creds = f.creds();
    creds.auth = vec![AuthMethod::Password, AuthMethod::Kbdint];
    let s = Session::connect(&creds, &f.opts(HostKeyPolicy::AcceptNew)).await.unwrap();
    assert_eq!(run(&s, "echo kbd", None).await.1, b"kbd\n");
}

#[tokio::test]
async fn key_auth_plain_and_encrypted() {
    let key = new_key();
    let f = start(Behaviour { authorized_key: Some(key.public_key().clone()), allow_password: false, kbdint: false }).await;
    let mut creds = f.creds();
    creds.auth = vec![AuthMethod::Key];
    creds.pass = None;
    creds.key = Some(SecretString::from(key_text(&key)));
    let s = Session::connect(&creds, &f.opts(HostKeyPolicy::AcceptNew)).await.unwrap();
    assert_eq!(run(&s, "echo key", None).await.1, b"key\n");

    let locked = key.encrypt(&mut rand::rng(), "pass phrase").unwrap();
    creds.key = Some(SecretString::from(key_text(&locked)));
    creds.key_pass = Some(SecretString::from("pass phrase"));
    let s = Session::connect(&creds, &f.opts(HostKeyPolicy::AcceptNew)).await.unwrap();
    assert_eq!(run(&s, "echo locked", None).await.1, b"locked\n");

    creds.key_pass = Some(SecretString::from("wrong"));
    let err = Session::connect(&creds, &f.opts(HostKeyPolicy::AcceptNew)).await.err().unwrap();
    assert_eq!(err.code(), "key_unusable");
}

#[tokio::test]
async fn strict_policy_rejects_an_unknown_host_key_and_names_its_fingerprint() {
    let f = start(Behaviour::default()).await;
    let err = Session::connect(&f.creds(), &f.opts(HostKeyPolicy::Strict)).await.err().unwrap();
    assert_eq!(err.code(), "host_key_unknown");
    let fingerprint = f.host_key.fingerprint(russh::keys::HashAlg::Sha256).to_string();
    assert!(err.to_string().contains(&fingerprint), "{err}");
    assert!(!f.known_hosts.exists());
}

#[tokio::test]
async fn accept_new_learns_the_key_and_strict_then_succeeds() {
    let f = start(Behaviour::default()).await;
    Session::connect(&f.creds(), &f.opts(HostKeyPolicy::AcceptNew)).await.unwrap();
    assert!(std::fs::read_to_string(&f.known_hosts).unwrap().contains("ssh-ed25519"));
    Session::connect(&f.creds(), &f.opts(HostKeyPolicy::Strict)).await.unwrap();
}

#[tokio::test]
async fn a_changed_host_key_is_a_hard_failure_even_with_accept_new() {
    let first = start(Behaviour::default()).await;
    Session::connect(&first.creds(), &first.opts(HostKeyPolicy::AcceptNew)).await.unwrap();

    let impostor = start(Behaviour::default()).await;
    let mut creds = impostor.creds();
    creds.port = impostor.addr.port();
    let mut opts = impostor.opts(HostKeyPolicy::AcceptNew);
    opts.known_hosts = first.known_hosts.clone();
    // the known_hosts entry is keyed by host and port, so mirror the original port
    let recorded = std::fs::read_to_string(&first.known_hosts).unwrap();
    let rewritten = recorded.replace(&format!("[127.0.0.1]:{}", first.addr.port()), &format!("[127.0.0.1]:{}", impostor.addr.port()));
    std::fs::write(&opts.known_hosts, rewritten).unwrap();

    for policy in [HostKeyPolicy::Strict, HostKeyPolicy::AcceptNew] {
        opts.policy = policy;
        let err = Session::connect(&creds, &opts).await.err().unwrap();
        assert_eq!(err.code(), "host_key_changed", "{policy:?}");
    }
}

#[tokio::test]
async fn a_silent_server_hits_the_connect_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _held = listener.accept().await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let f = start(Behaviour::default()).await;
    let mut creds = f.creds();
    creds.port = port;
    let mut opts = f.opts(HostKeyPolicy::AcceptNew);
    opts.connect_timeout = Duration::from_millis(500);
    let err = Session::connect(&creds, &opts).await.err().unwrap();
    assert_eq!(err.code(), "connect_timeout");
}

#[tokio::test]
async fn nothing_listening_is_a_connect_error() {
    let f = start(Behaviour::default()).await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let mut creds = f.creds();
    creds.port = port;
    let err = Session::connect(&creds, &f.opts(HostKeyPolicy::AcceptNew)).await.err().unwrap();
    assert_eq!(err.code(), "connect_failed");
}

#[tokio::test]
async fn no_credentials_is_reported_before_connecting() {
    let f = start(Behaviour::default()).await;
    let mut creds = f.creds();
    creds.auth.clear();
    let err = Session::connect(&creds, &f.opts(HostKeyPolicy::AcceptNew)).await.err().unwrap();
    assert_eq!(err.code(), "no_credentials");
}
