mod common;

use std::path::{Path, PathBuf};

use common::*;
use serde_json::Value;
use sshhh::hostkeys;

const EXE: &str = env!("CARGO_BIN_EXE_sshhh");

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim()).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", self.stdout))
    }

    fn error_code(&self) -> String {
        let line = self.stderr.lines().rev().find(|l| l.starts_with('{') && l.contains("\"error\"")).unwrap_or_else(|| panic!("no error line in: {}", self.stderr));
        serde_json::from_str::<Value>(line).unwrap()["error"].as_str().unwrap().to_string()
    }
}

struct Cli {
    f: Fixture,
    home: tempfile::TempDir,
    work: tempfile::TempDir,
}

impl Drop for Cli {
    fn drop(&mut self) {
        let mut stop = std::process::Command::new(EXE);
        stop.args(["daemon", "stop"]).current_dir(self.work.path());
        self.isolate_std(&mut stop);
        let _ = stop.output();
    }
}

impl Cli {
    async fn new(env_extra: &str) -> Cli {
        let f = start(Behaviour::default()).await;
        let cli = Cli { f, home: tempfile::tempdir().unwrap(), work: tempfile::tempdir().unwrap() };
        let text = format!("HOST=127.0.0.1\nPORT={}\nUSER={USER}\nPASS=\"{PASSWORD}\"\n{env_extra}", cli.f.addr.port());
        std::fs::write(cli.work.path().join(".env"), text).unwrap();
        cli
    }

    fn isolate_std(&self, cmd: &mut std::process::Command) {
        for (key, _) in std::env::vars() {
            if key.starts_with("SSHHH_") || key == "SSH_SERVER" || key == "BW_SESSION" {
                cmd.env_remove(key);
            }
        }
        cmd.env("HOME", self.home.path()).env("USERPROFILE", self.home.path());
    }

    fn command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(EXE);
        for (key, _) in std::env::vars() {
            if key.starts_with("SSHHH_") || key == "SSH_SERVER" || key == "BW_SESSION" {
                cmd.env_remove(key);
            }
        }
        cmd.env("HOME", self.home.path()).env("USERPROFILE", self.home.path()).current_dir(self.work.path()).stdin(std::process::Stdio::null());
        cmd
    }

    async fn run(&self, args: &[&str]) -> Out {
        let output = self.command().args(args).output().await.unwrap();
        Out {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.work.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn known_hosts(&self) -> PathBuf {
        hostkeys::default_path(self.home.path())
    }
}

fn str_of(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[tokio::test]
async fn version_and_help_succeed_without_any_configuration() {
    let cli = Cli::new("").await;
    std::fs::remove_file(cli.work.path().join(".env")).unwrap();
    for flag in ["--version", "--help"] {
        let out = cli.run(&[flag]).await;
        assert_eq!(out.code, 0, "{flag}: {}", out.stderr);
        assert!(out.stdout.contains("sshhh"), "{flag}");
    }
    let out = cli.run(&["put", "--help"]).await;
    assert_eq!(out.code, 0);
}

#[tokio::test]
async fn a_command_streams_its_output_and_passes_the_remote_status_through() {
    let cli = Cli::new("").await;
    let out = cli.run(&["--no-daemon", "--accept-new", "echo hi"]).await;
    assert_eq!((out.code, out.stdout.as_str()), (0, "hi\n"), "{}", out.stderr);
    let out = cli.run(&["--no-daemon", "--accept-new", "exit 3"]).await;
    assert_eq!(out.code, 3);
    let out = cli.run(&["--no-daemon", "--accept-new", "both"]).await;
    assert_eq!((out.code, out.stdout.as_str(), out.stderr.as_str()), (7, "out\n", "err\n"));
}

#[tokio::test]
async fn several_words_are_quoted_one_by_one_and_a_single_word_is_verbatim() {
    let cli = Cli::new("").await;
    let out = cli.run(&["--no-daemon", "--accept-new", "echo", "a b"]).await;
    assert_eq!(out.stdout, "'a b'\n", "{}", out.stderr);
    let out = cli.run(&["--no-daemon", "--accept-new", "echo $HOME"]).await;
    assert_eq!(out.stdout, "$HOME\n");
}

#[tokio::test]
async fn json_output_is_one_envelope_with_the_status_and_both_streams() {
    let cli = Cli::new("").await;
    let out = cli.run(&["--no-daemon", "--accept-new", "--json", "both"]).await;
    assert_eq!(out.code, 7);
    let v = out.json();
    assert_eq!((v["alias"].as_str(), v["rc"].as_i64(), v["stdout"].as_str(), v["stderr"].as_str()), (Some("default"), Some(7), Some("out\n"), Some("err\n")));
    assert!(v["duration_ms"].is_u64());
}

#[tokio::test]
async fn flags_work_before_and_after_the_command_and_a_subcommand_name_needs_a_separator() {
    let cli = Cli::new("").await;
    let before = cli.run(&["--no-daemon", "--accept-new", "-s", "default", "echo", "x"]).await;
    let after = cli.run(&["--no-daemon", "--accept-new", "echo", "x", "-s", "default"]).await;
    assert_eq!(before.stdout, "x\n", "{}", before.stderr);
    assert_eq!(after.stdout, "x -s default\n", "a flag after the command belongs to the remote command: {}", after.stderr);
    let listed = cli.run(&["list", "--json"]).await;
    assert_eq!(listed.code, 0, "{}", listed.stderr);
    let separated = cli.run(&["--no-daemon", "--accept-new", "--", "echo", "list"]).await;
    assert_eq!(separated.stdout, "list\n", "{}", separated.stderr);
    let put_flag_last = cli.run(&["--no-daemon", "--accept-new", "ls", "/home/deploy", "--json"]).await;
    assert_eq!(put_flag_last.code, 0, "{}", put_flag_last.stderr);
}

#[tokio::test]
async fn usage_errors_are_json_with_exit_255() {
    let cli = Cli::new("").await;
    let out = cli.run(&[]).await;
    assert_eq!((out.code, out.error_code().as_str()), (255, "usage"));
    let out = cli.run(&["--no-such-flag"]).await;
    assert_eq!((out.code, out.error_code().as_str()), (255, "usage"));
    let out = cli.run(&["-s", "nobody", "--no-daemon", "echo", "x"]).await;
    assert_eq!((out.code, out.error_code().as_str()), (255, "unknown_server"));
}

#[tokio::test]
async fn file_commands_refuse_escalation_and_stdin_instead_of_ignoring_them() {
    let cli = Cli::new("").await;
    let local = cli.file("x.txt", b"x");
    let base = ["--no-daemon", "--accept-new", "--json"];
    for flag in ["--root", "--sudo", "--su", "--stdin"] {
        for words in [vec!["put", str_of(&local), "/home/deploy/x.txt"], vec!["get", "/home/deploy/x.txt", str_of(&local)], vec!["ls", "/home/deploy"], vec!["cat", "/home/deploy/x.txt"]] {
            let out = cli.run(&[&base[..], &[flag], &words[..]].concat()).await;
            assert_eq!((out.code, out.error_code().as_str()), (255, "usage"), "{flag} {words:?}: {}", out.stderr);
        }
    }
}

#[tokio::test]
async fn a_script_refuses_the_stdin_flag_instead_of_ignoring_it() {
    let cli = Cli::new("").await;
    let script = cli.file("s.sh", b"echo hi\n");
    let out = cli.run(&["--no-daemon", "--accept-new", "--json", "--stdin", "script", str_of(&script)]).await;
    assert_eq!((out.code, out.error_code().as_str()), (255, "usage"), "{}", out.stderr);
}

#[tokio::test]
async fn an_unknown_host_key_fails_until_trusted_and_a_wrong_fingerprint_trusts_nothing() {
    let cli = Cli::new("").await;
    let out = cli.run(&["--no-daemon", "echo hi"]).await;
    assert_eq!((out.code, out.error_code().as_str()), (255, "host_key_unknown"));

    let wrong = cli.run(&["trust", "--fingerprint", "SHA256:nope"]).await;
    assert_eq!((wrong.code, wrong.error_code().as_str()), (255, "usage"));
    assert!(!cli.known_hosts().exists());

    let fingerprint = hostkeys::fingerprint(&cli.f.host_key);
    let out = cli.run(&["trust", "--fingerprint", &fingerprint]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let out = cli.run(&["--no-daemon", "echo hi"]).await;
    assert_eq!((out.code, out.stdout.as_str()), (0, "hi\n"), "{}", out.stderr);
    let again = cli.run(&["trust", "--fingerprint", &fingerprint]).await;
    assert_eq!(again.code, 0);

    let out = cli.run(&["forget"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let out = cli.run(&["--no-daemon", "echo hi"]).await;
    assert_eq!(out.error_code(), "host_key_unknown");
}

#[tokio::test]
async fn trust_without_a_terminal_or_fingerprint_refuses() {
    let cli = Cli::new("").await;
    let out = cli.run(&["trust"]).await;
    assert_eq!((out.code, out.error_code().as_str()), (255, "usage"));
    assert!(!cli.known_hosts().exists());
}

#[tokio::test]
async fn files_round_trip_with_verification_and_stay_inside_the_json_contract() {
    let cli = Cli::new("").await;
    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let local = cli.file("data.bin", &payload);
    let base = ["--no-daemon", "--accept-new", "--json"];

    let out = cli.run(&[&base[..], &["put", "--verify", str_of(&local), "/home/deploy/data.bin"]].concat()).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let v = out.json();
    assert_eq!((v["bytes"].as_u64(), v["verified"].as_bool(), v["alias"].as_str()), (Some(payload.len() as u64), Some(true), Some("default")));
    assert_eq!(cli.f.fs.read("/home/deploy/data.bin"), payload);

    let back = cli.work.path().join("back.bin");
    let out = cli.run(&[&base[..], &["get", "--verify", "~/data.bin", str_of(&back)]].concat()).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(std::fs::read(&back).unwrap(), payload);

    let out = cli.run(&[&base[..], &["ls", "/home/deploy"]].concat()).await;
    let v = out.json();
    assert_eq!(v["entries"][0]["name"].as_str(), Some("data.bin"));

    cli.f.fs.write("/home/deploy/note.txt", "héllo\n".as_bytes());
    let out = cli.run(&[&base[..], &["cat", "/home/deploy/note.txt"]].concat()).await;
    assert_eq!(out.json()["stdout"].as_str(), Some("héllo\n"));
    cli.f.fs.write("/home/deploy/blob", &[0xff, 0xfe, 0x00]);
    let out = cli.run(&[&base[..], &["cat", "/home/deploy/blob"]].concat()).await;
    assert_eq!((out.code, out.error_code().as_str()), (255, "usage"));
}

#[tokio::test]
async fn plain_file_commands_print_a_summary_and_cat_streams_raw_bytes() {
    let cli = Cli::new("").await;
    let local = cli.file("up.txt", b"hello");
    let out = cli.run(&["--no-daemon", "--accept-new", "put", str_of(&local), "/home/deploy/up.txt"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("5 bytes"), "{}", out.stdout);
    let out = cli.run(&["--no-daemon", "--accept-new", "cat", "/home/deploy/up.txt"]).await;
    assert_eq!(out.stdout, "hello");
    let out = cli.run(&["--no-daemon", "--accept-new", "ls", "/home/deploy"]).await;
    assert!(out.stdout.contains("up.txt"), "{}", out.stdout);
}

#[tokio::test]
async fn readonly_in_the_env_file_cannot_be_overridden_from_the_command_line() {
    let cli = Cli::new("READONLY=true\nALLOW_COMMANDS=echo\n").await;
    let local = cli.file("up.txt", b"x");
    let out = cli.run(&["--no-daemon", "--accept-new", "put", str_of(&local), "/home/deploy/up.txt"]).await;
    assert_eq!((out.code, out.error_code().as_str()), (255, "policy_denied"));
    assert!(cli.f.fs.names("/home/deploy").is_empty());
    let out = cli.run(&["--no-daemon", "--accept-new", "rm -rf /home/deploy"]).await;
    assert_eq!(out.error_code(), "policy_denied");
    let out = cli.run(&["--no-daemon", "--accept-new", "echo", "fine"]).await;
    assert_eq!((out.code, out.stdout.as_str()), (0, "fine\n"), "{}", out.stderr);
}

#[tokio::test]
async fn a_script_runs_through_the_shell_reader_and_is_denied_under_a_policy() {
    let cli = Cli::new("").await;
    let script = cli.file("job.sh", b"echo hi\n");
    let out = cli.run(&["--no-daemon", "--accept-new", "script", str_of(&script), "a b"]).await;
    assert_eq!(out.code, 127, "the fixture knows no shell: {}", out.stderr);

    let locked = Cli::new("DENY_COMMANDS=rm\n").await;
    let script = locked.file("job.sh", b"echo hi\n");
    let out = locked.run(&["--no-daemon", "--accept-new", "script", str_of(&script)]).await;
    assert_eq!((out.code, out.error_code().as_str()), (255, "policy_denied"));
}

#[tokio::test]
async fn fan_out_collects_every_server_and_reports_failures_per_alias() {
    let cli = Cli::new("").await;
    let port = cli.f.addr.port();
    let more = format!("ALPHA_HOST=127.0.0.1\nALPHA_PORT={port}\nALPHA_USER={USER}\nALPHA_PASS=\"{PASSWORD}\"\nBROKEN_HOST=127.0.0.1\nBROKEN_PORT=1\nBROKEN_USER={USER}\nBROKEN_PASS=x\n");
    let mut text = std::fs::read_to_string(cli.work.path().join(".env")).unwrap();
    text.push_str(&more);
    std::fs::write(cli.work.path().join(".env"), text).unwrap();

    let out = cli.run(&["--no-daemon", "--accept-new", "--json", "--on", "default,alpha", "echo hi"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let all = out.json();
    let aliases: Vec<&str> = all.as_array().unwrap().iter().map(|v| v["alias"].as_str().unwrap()).collect();
    assert_eq!(aliases, ["default", "alpha"]);
    assert!(all.as_array().unwrap().iter().all(|v| v["stdout"] == "hi\n"));

    let out = cli.run(&["--no-daemon", "--accept-new", "--json", "--on", "alpha,broken", "echo hi"]).await;
    assert_eq!(out.code, 255);
    let all = out.json();
    assert_eq!(all[0]["rc"], 0);
    assert_eq!((all[1]["alias"].as_str(), all[1]["error"].as_str()), (Some("broken"), Some("connect_failed")));

    let out = cli.run(&["--no-daemon", "--accept-new", "--on", "default,alpha", "echo hi"]).await;
    assert!(out.stdout.contains("== default") && out.stdout.contains("== alpha"), "{}", out.stdout);
}

#[tokio::test]
async fn list_shows_how_each_secret_is_stored_and_never_a_value() {
    let cli = Cli::new("ROOT_PASS=bw://item/field\nREADONLY=true\n").await;
    let out = cli.run(&["list"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    for text in [&out.stdout, &out.stderr] {
        assert!(!text.contains(PASSWORD));
    }
    assert!(out.stdout.contains("pass=literal") && out.stdout.contains("root_pass=bw://item/field") && out.stdout.contains("readonly"), "{}", out.stdout);
    let out = cli.run(&["list", "--json"]).await;
    assert!(!out.stdout.contains(PASSWORD));
    let v = out.json();
    assert_eq!((v["servers"][0]["pass"].as_str(), v["servers"][0]["policy"]["readonly"].as_bool()), (Some("literal"), Some(true)));
}

#[tokio::test]
async fn doctor_reports_checks_and_fails_when_something_is_wrong() {
    let cli = Cli::new("").await;
    let out = cli.run(&["doctor", "--json"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let checks = out.json();
    let ids: Vec<&str> = checks.as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    for id in ["credentials_resolve", "host_key_unknown", "policy", "daemon_stopped"] {
        assert!(ids.contains(&id), "{id} missing from {ids:?}");
    }
    assert!(!out.stdout.contains(PASSWORD));

    std::fs::write(cli.work.path().join(".env"), "HOST=127.0.0.1\nUSER=deploy\nPASS=env://SSHHH_TEST_UNSET_VARIABLE\n").unwrap();
    let out = cli.run(&["doctor"]).await;
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(out.stdout.contains("FAIL"), "{}", out.stdout);
}

#[tokio::test]
async fn import_rewrites_legacy_key_names_only_and_dry_run_writes_nothing() {
    let cli = Cli::new("").await;
    let legacy = "# servers\nZEUS_IP=1.2.3.4\nZEUS_PASSWORD=\"a b\"\n";
    let path = cli.file("legacy.env", legacy.as_bytes());

    let out = cli.run(&["import", "--dry-run", str_of(&path)]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("ZEUS_IP -> ZEUS_HOST"), "{}", out.stdout);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), legacy);

    let out = cli.run(&["import", str_of(&path)]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# servers\nZEUS_HOST=1.2.3.4\nZEUS_PASS=\"a b\"\n");
    let names: Vec<String> = std::fs::read_dir(cli.work.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().all(|n| !n.contains("sshhh-tmp")), "{names:?}");

    let out = cli.run(&["import", "--json", str_of(&path)]).await;
    assert_eq!((out.json()["written"].as_bool(), out.json()["changes"].as_array().map(Vec::len)), (Some(false), Some(0)));
}

#[tokio::test]
async fn legacy_keys_still_work_and_warn_on_stderr() {
    let cli = Cli::new("").await;
    let port = cli.f.addr.port();
    std::fs::write(cli.work.path().join(".env"), format!("SSH_HOST=127.0.0.1\nSSH_PORT={port}\nSSH_USER={USER}\nSSH_PASS=\"{PASSWORD}\"\n")).unwrap();
    let out = cli.run(&["--no-daemon", "--accept-new", "echo hi"]).await;
    assert_eq!((out.code, out.stdout.as_str()), (0, "hi\n"), "{}", out.stderr);
    assert!(out.stderr.contains("deprecated_key"), "{}", out.stderr);
}

#[tokio::test]
async fn the_daemon_starts_on_demand_holds_one_session_and_stops() {
    let cli = Cli::new("").await;
    let out = cli.run(&["--accept-new", "echo hi"]).await;
    assert_eq!((out.code, out.stdout.as_str()), (0, "hi\n"), "{}", out.stderr);
    let out = cli.run(&["--accept-new", "--json", "echo", "again"]).await;
    assert_eq!(out.json()["reconnected"].as_bool(), Some(false), "{}", out.stdout);

    let out = cli.run(&["daemon", "status", "--json"]).await;
    let v = out.json();
    assert_eq!((v["running"].as_bool(), v["status"]["sessions"].as_array().map(Vec::len)), (Some(true), Some(1)), "{}", out.stdout);
    assert!(!out.stdout.contains(PASSWORD));

    let out = cli.run(&["daemon", "stop"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    for _ in 0..50 {
        let status = cli.run(&["daemon", "status", "--json"]).await;
        if status.json()["running"] == false {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the daemon is still running after stop");
}

#[tokio::test]
async fn an_explicit_env_file_is_the_only_source() {
    let cli = Cli::new("").await;
    let port = cli.f.addr.port();
    let other = cli.file("other.env", format!("HOST=127.0.0.1\nPORT={port}\nUSER={USER}\nPASS=\"{PASSWORD}\"\nREADONLY=true\n").as_bytes());
    let local = cli.file("up.txt", b"x");
    let out = cli.run(&["--no-daemon", "--accept-new", "--env", str_of(&other), "put", str_of(&local), "/home/deploy/up.txt"]).await;
    assert_eq!(out.error_code(), "policy_denied");
    let out = cli.run(&["--no-daemon", "--accept-new", "put", str_of(&local), "/home/deploy/up.txt"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
}
