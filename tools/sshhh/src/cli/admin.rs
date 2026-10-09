use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use sshhh::config::{Config, Server, Settings};
use sshhh::daemon::{self, Status};
use sshhh::doctor::{self, Check, Level};
use sshhh::error::Error;
use sshhh::hostkeys::{self, Known};
use sshhh::i18n::Catalog;
use sshhh::import;
use sshhh::private;
use sshhh::resolve::Resolver;
use sshhh::secrets::{self, parse_reference};
use sshhh::spec::ServerSpec;

use super::run::{client, context};
use super::{DaemonAction, Global, Setup, home, load};

fn chosen<'a>(config: &'a Config, global: &Global, alias: Option<&str>) -> Result<Vec<&'a Server>, Error> {
    if alias.is_some() || global.server.is_some() || !global.on.is_empty() || global.all {
        return super::run::select(config, global, alias);
    }
    if config.servers.is_empty() {
        return Err(Error::NoConfig { searched: String::new() });
    }
    Ok(config.servers.values().collect())
}

fn one<'a>(setup: &'a Setup, global: &Global, alias: Option<String>) -> Result<&'a Server, Error> {
    setup.config.select(alias.as_deref().or(global.server.as_deref()))
}

fn print_json(value: &Value) {
    println!("{value}");
}

fn secret_kind(value: &Option<String>) -> Option<String> {
    let value = value.as_ref()?;
    Some(match parse_reference(value) {
        Ok(Some(_)) => value.clone(),
        Ok(None) => "literal".to_string(),
        Err(_) => "invalid reference".to_string(),
    })
}

fn key_kind(value: &Option<String>) -> Option<String> {
    let value = value.as_ref()?;
    if value.contains("-----BEGIN") { Some("inline".to_string()) } else { Some(value.clone()) }
}

fn name_of<T: serde::Serialize>(value: &Option<T>) -> Option<String> {
    value.as_ref().and_then(|v| serde_json::to_value(v).ok()).and_then(|v| v.as_str().map(str::to_string))
}

fn describe(spec: &ServerSpec, config: &Config) -> Value {
    let auth: Vec<String> = spec.auth.iter().flatten().filter_map(|a| serde_json::to_value(a).ok()).filter_map(|v| v.as_str().map(str::to_string)).collect();
    let port = spec.port.unwrap_or(22);
    json!({
        "alias": spec.alias,
        "default": config.default_alias.as_deref() == Some(spec.alias.as_str()),
        "host": spec.host,
        "port": port,
        "user": spec.user,
        "auth": auth,
        "escalate": name_of(&spec.escalate),
        "pass": secret_kind(&spec.pass),
        "key": key_kind(&spec.key),
        "key_pass": secret_kind(&spec.key_pass),
        "root_user": spec.root_user,
        "root_pass": secret_kind(&spec.root_pass),
        "sudo_pass": secret_kind(&spec.sudo_pass),
        "vault": spec.vault,
        "policy": spec.policy,
    })
}

pub fn list(global: &Global, tr: &Catalog) -> Result<u8, Error> {
    let setup = load(global)?;
    let servers = chosen(&setup.config, global, None)?;
    let described: Vec<Value> = servers.iter().map(|s| describe(&ServerSpec::from_server(s), &setup.config)).collect();
    if global.json {
        print_json(&json!({
            "servers": described,
            "incomplete": setup.config.incomplete,
            "sources": setup.config.sources.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        }));
        return Ok(0);
    }
    for server in &described {
        let text = |name: &str| server.get(name).and_then(Value::as_str).unwrap_or("-").to_string();
        let marker = if server["default"].as_bool() == Some(true) { "*" } else { "" };
        let mut details = Vec::new();
        for name in ["pass", "key", "key_pass", "root_pass", "sudo_pass", "vault"] {
            if let Some(value) = server.get(name).and_then(Value::as_str) {
                details.push(format!("{name}={value}"));
            }
        }
        let policy = &server["policy"];
        let count = |name: &str| policy[name].as_array().map_or(0, Vec::len);
        let mut rules = Vec::new();
        if policy["readonly"].as_bool() == Some(true) {
            rules.push("readonly".to_string());
        }
        for name in ["allow_commands", "deny_commands", "allow_paths", "deny_paths"] {
            if count(name) > 0 {
                rules.push(format!("{name}={}", count(name)));
            }
        }
        println!(
            "{}",
            tr.t(
                "list.line",
                &[
                    ("alias", &format!("{}{marker}", text("alias"))),
                    ("user", &text("user")),
                    ("host", &text("host")),
                    ("port", &server["port"]),
                    ("auth", &server["auth"].as_array().map_or_else(String::new, |a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","))),
                    ("details", &details.join(" ")),
                    ("policy", &rules.join(" ")),
                ]
            )
        );
    }
    for alias in &setup.config.incomplete {
        println!("{}", tr.t("list.incomplete", &[("alias", alias)]));
    }
    for source in &setup.config.sources {
        println!("{}", tr.t("list.source", &[("path", &source.display())]));
    }
    Ok(0)
}

fn check_text(check: &Check, tr: &Catalog) -> String {
    let args: &[(&str, &dyn std::fmt::Display)] = &[("subject", &check.subject), ("detail", &check.detail)];
    match check.id {
        "env_permissions" => tr.t("doctor.env_permissions", args),
        "env_tracked" => tr.t("doctor.env_tracked", args),
        "env_not_ignored" => tr.t("doctor.env_not_ignored", args),
        "bw_found" => tr.t("doctor.bw_found", args),
        "bw_server" => tr.t("doctor.bw_server", args),
        "bw_wrong_server" => tr.t("doctor.bw_wrong_server", args),
        "bw_unlocked" => tr.t("doctor.bw_unlocked", args),
        "bw_locked" => tr.t("doctor.bw_locked", args),
        "bw_not_logged_in" => tr.t("doctor.bw_not_logged_in", args),
        "bw_state_unknown" => tr.t("doctor.bw_state_unknown", args),
        "bw_unavailable" => tr.t("doctor.bw_unavailable", args),
        "credentials_resolve" => tr.t("doctor.credentials_resolve", args),
        "credentials_unresolved" => tr.t("doctor.credentials_unresolved", args),
        "secret_too_short" => tr.t("doctor.secret_too_short", args),
        "host_key_known" => tr.t("doctor.host_key_known", args),
        "host_key_unknown" => tr.t("doctor.host_key_unknown", args),
        "host_key_unreadable" => tr.t("doctor.host_key_unreadable", args),
        "escalation_unresolved" => tr.t("doctor.escalation_unresolved", args),
        "policy" => tr.t("doctor.policy", args),
        "policy_unresolved" => tr.t("doctor.policy_unresolved", args),
        "key_permissions" => tr.t("doctor.key_permissions", args),
        "daemon_running" => tr.t("doctor.daemon_running", args),
        "daemon_stopped" => tr.t("doctor.daemon_stopped", args),
        other => format!("[{other}] {} {}", check.subject, check.detail),
    }
}

fn level_text(level: Level, tr: &Catalog) -> String {
    match level {
        Level::Ok => tr.t("doctor.level_ok", &[]),
        Level::Warn => tr.t("doctor.level_warn", &[]),
        Level::Fail => tr.t("doctor.level_fail", &[]),
    }
}

pub async fn doctor(global: &Global, alias: Option<String>, tr: &Catalog) -> Result<u8, Error> {
    let setup = load(global)?;
    let targets = chosen(&setup.config, global, alias.as_deref())?;
    let specs: Vec<ServerSpec> = targets.iter().map(|s| ServerSpec::from_server(s)).collect();
    let ctx = context(&setup, &specs, global);
    let status: Option<Status> = client(&setup)?.status().await?;
    let known_hosts = hostkeys::default_path(&setup.home);
    let checks = doctor::run(&setup.config, &targets, &ctx, status.as_ref(), &known_hosts).await;
    if global.json {
        print_json(&serde_json::to_value(&checks).map_err(|e| Error::Io(e.to_string()))?);
    } else {
        for check in &checks {
            println!("{}", tr.t("doctor.line", &[("level", &level_text(check.level, tr)), ("text", &check_text(check, tr))]));
        }
    }
    Ok(u8::from(checks.iter().any(|c| c.level == Level::Fail)))
}

async fn endpoint(setup: &Setup, global: &Global, server: &Server) -> Result<(String, u16), Error> {
    let spec = ServerSpec::from_server(server);
    let ctx = context(setup, std::slice::from_ref(&spec), global);
    Resolver::new(&ctx.settings, ctx.env.clone(), ctx.home.clone(), ctx.cwd.clone()).endpoint(server).await
}

pub async fn trust(global: &Global, alias: Option<String>, fingerprint: Option<String>, tr: &Catalog) -> Result<u8, Error> {
    let setup = load(global)?;
    let server = one(&setup, global, alias)?;
    let (host, port) = endpoint(&setup, global, server).await?;
    let path = hostkeys::default_path(&setup.home);
    let key = hostkeys::probe(&host, port, Duration::from_secs(global.connect_timeout)).await?;
    let seen = hostkeys::fingerprint(&key);
    let pattern = hostkeys::host_pattern(&host, port);
    if hostkeys::check(&path, &host, port, &key)? == Known::Matches {
        report(global, tr, json!({"alias": server.alias, "host": pattern, "fingerprint": seen, "learned": false}), "trust.already", &[("host", &pattern), ("fingerprint", &seen)]);
        return Ok(0);
    }
    match fingerprint {
        Some(expected) if expected.trim() == seen => {}
        Some(expected) => {
            return Err(Error::Usage(format!("{pattern} presents {seen}, not the expected {}; nothing was trusted", expected.trim())));
        }
        None => {
            if !std::io::stdin().is_terminal() {
                return Err(Error::Usage(format!("{pattern} presents {seen}; run this on a terminal to confirm, or pass --fingerprint {seen} after checking it out of band")));
            }
            eprint!("{} ", tr.t("trust.prompt", &[("host", &pattern), ("fingerprint", &seen)]));
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            std::io::stdin().lock().read_line(&mut answer).map_err(|e| Error::Io(format!("cannot read the answer: {e}")))?;
            if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                return Err(Error::Usage(format!("{pattern} was not trusted")));
            }
        }
    }
    hostkeys::learn(&path, &host, port, &key)?;
    report(global, tr, json!({"alias": server.alias, "host": pattern, "fingerprint": seen, "learned": true}), "trust.done", &[("host", &pattern), ("fingerprint", &seen)]);
    Ok(0)
}

pub async fn forget(global: &Global, alias: Option<String>, tr: &Catalog) -> Result<u8, Error> {
    let setup = load(global)?;
    let server = one(&setup, global, alias)?;
    let (host, port) = endpoint(&setup, global, server).await?;
    let pattern = hostkeys::host_pattern(&host, port);
    let removed = hostkeys::forget(&hostkeys::default_path(&setup.home), &host, port)?;
    let key = if removed == 0 { "forget.none" } else { "forget.done" };
    report(global, tr, json!({"alias": server.alias, "host": pattern, "removed": removed}), key, &[("host", &pattern), ("count", &removed)]);
    Ok(0)
}

fn report(global: &Global, tr: &Catalog, value: Value, key: &str, args: &[(&str, &dyn std::fmt::Display)]) {
    if global.json {
        print_json(&value);
    } else {
        println!("{}", tr.t(key, args));
    }
}

pub async fn daemon(action: DaemonAction, global: &Global, tr: &Catalog) -> Result<u8, Error> {
    let home = home()?;
    match action {
        DaemonAction::Run { idle } => {
            daemon::serve(&home, Duration::from_secs(idle)).await?;
            Ok(0)
        }
        DaemonAction::Status => {
            let exe = std::env::current_exe().map_err(|e| Error::Io(format!("cannot locate the sshhh executable: {e}")))?;
            let status = daemon::Client::new(&home, exe).status().await?;
            if global.json {
                print_json(&json!({ "running": status.is_some(), "status": status }));
                return Ok(0);
            }
            match status {
                None => println!("{}", tr.t("daemon.stopped", &[])),
                Some(s) => {
                    println!(
                        "{}",
                        tr.t(
                            "daemon.running",
                            &[("pid", &s.pid), ("uptime", &s.uptime_secs), ("idle", &s.idle_exit_secs), ("bw", &s.bw_session), ("count", &s.sessions.len())]
                        )
                    );
                    for session in &s.sessions {
                        println!("{}", tr.t("daemon.session", &[("alias", &session.alias), ("user", &session.user), ("idle", &session.idle_secs)]));
                    }
                }
            }
            Ok(0)
        }
        DaemonAction::Stop => {
            let exe = std::env::current_exe().map_err(|e| Error::Io(format!("cannot locate the sshhh executable: {e}")))?;
            let stopped = daemon::Client::new(&home, exe).stop().await?;
            report(global, tr, json!({ "stopped": stopped }), if stopped { "daemon.stopping" } else { "daemon.stopped" }, &[]);
            Ok(0)
        }
    }
}

pub async fn unlock_bw(global: &Global, tr: &Catalog) -> Result<u8, Error> {
    let home = home()?;
    let settings = match load(global) {
        Ok(setup) => setup.config.settings,
        Err(Error::NoConfig { .. }) => Settings::default(),
        Err(e) => return Err(e),
    };
    let session = secrets::unlock_bitwarden(&settings).await?;
    let exe = std::env::current_exe().map_err(|e| Error::Io(format!("cannot locate the sshhh executable: {e}")))?;
    daemon::Client::new(&home, exe).set_bw_session(&session).await?;
    report(global, tr, json!({ "unlocked": true }), "unlock.done", &[]);
    Ok(0)
}

pub fn import(global: &Global, path: &Path, dry_run: bool, tr: &Catalog) -> Result<u8, Error> {
    let file = if path.is_dir() { path.join(".env") } else { path.to_path_buf() };
    let text = std::fs::read_to_string(&file).map_err(|e| Error::EnvRead { path: file.clone(), reason: e.kind().to_string() })?;
    let plan = import::plan(&text, &file)?;
    let written = !dry_run && !plan.changes.is_empty();
    if written {
        private::write_private(&file, plan.text.as_bytes()).map_err(|e| Error::Io(format!("{}: {}", file.display(), e.kind())))?;
    }
    if global.json {
        print_json(&json!({ "file": file.display().to_string(), "changes": plan.changes, "written": written }));
        return Ok(0);
    }
    for change in &plan.changes {
        println!("{}", tr.t("import.change", &[("line", &change.line), ("from", &change.from), ("to", &change.to)]));
    }
    let key = match (plan.changes.is_empty(), dry_run) {
        (true, _) => "import.nothing",
        (false, true) => "import.dry_run",
        (false, false) => "import.written",
    };
    println!("{}", tr.t(key, &[("file", &file.display()), ("count", &plan.changes.len())]));
    Ok(0)
}
