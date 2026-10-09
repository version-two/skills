use std::path::{Path, PathBuf};

use secrecy::ExposeSecret;
use serde::Serialize;

use crate::config::{Config, Server};
use crate::daemon::Status;
use crate::error::Error;
use crate::hostkeys;
use crate::resolve::Resolver;
use crate::secrets::{Secrets, parse_reference};
use crate::spec::{Context, ServerSpec};

const MIN_REDACTABLE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub level: Level,
    pub subject: String,
    pub detail: String,
}

fn check(id: &'static str, level: Level, subject: impl Into<String>, detail: impl Into<String>) -> Check {
    Check { id, level, subject: subject.into(), detail: detail.into() }
}

/// Never prints a secret: references are resolved to prove they work and only their outcome is
/// reported.
pub async fn run(config: &Config, targets: &[&Server], ctx: &Context, daemon: Option<&Status>, known_hosts: &Path) -> Vec<Check> {
    let mut out = Vec::new();
    let specs: Vec<ServerSpec> = targets.iter().map(|s| ServerSpec::from_server(s)).collect();

    for source in &config.sources {
        out.extend(permissions("env_permissions", source));
    }
    if specs.iter().any(|s| !s.literal_secrets().is_empty()) {
        for source in &config.sources {
            match git_exposure(source) {
                Some(Exposure::Tracked) => out.push(check("env_tracked", Level::Fail, source.display().to_string(), "git tracks this file")),
                Some(Exposure::NotIgnored) => out.push(check("env_not_ignored", Level::Warn, source.display().to_string(), "git does not ignore this file")),
                None => {}
            }
        }
    }

    if specs.iter().any(ServerSpec::uses_bitwarden) {
        out.extend(bitwarden(ctx).await);
    }

    for (server, spec) in targets.iter().zip(&specs) {
        let alias = server.alias.as_str();
        let resolver = Resolver::new(&ctx.settings, ctx.env.clone(), ctx.home.clone(), ctx.cwd.clone());
        match resolver.credentials(server).await {
            Ok(creds) => {
                out.push(check("credentials_resolve", Level::Ok, alias, format!("{}@{}:{}", creds.user, creds.host, creds.port)));
                let secrets = [("PASS", &creds.pass), ("KEY_PASS", &creds.key_pass)];
                for (name, secret) in secrets {
                    if secret.as_ref().is_some_and(|s| (1..MIN_REDACTABLE).contains(&s.expose_secret().chars().count())) {
                        out.push(check("secret_too_short", Level::Warn, alias, name));
                    }
                }
                match hostkeys::has_entry(known_hosts, &creds.host, creds.port) {
                    Ok(true) => out.push(check("host_key_known", Level::Ok, alias, hostkeys::host_pattern(&creds.host, creds.port))),
                    Ok(false) => out.push(check("host_key_unknown", Level::Warn, alias, hostkeys::host_pattern(&creds.host, creds.port))),
                    Err(e) => out.push(check("host_key_unreadable", Level::Fail, alias, e.to_string())),
                }
            }
            Err(e) => out.push(failure("credentials_unresolved", alias, &e)),
        }
        match resolver.escalation(server).await {
            Ok(escalation) => {
                for (name, secret) in [("ROOT_PASS", &escalation.root_pass), ("SUDO_PASS", &escalation.sudo_pass)] {
                    if secret.as_ref().is_some_and(|s| (1..MIN_REDACTABLE).contains(&s.expose_secret().chars().count())) {
                        out.push(check("secret_too_short", Level::Warn, alias, name));
                    }
                }
            }
            Err(e) => out.push(failure("escalation_unresolved", alias, &e)),
        }
        match resolver.vault_policy(server).await {
            Ok(vault) => {
                let mut policy = spec.policy.clone();
                policy.merge(&vault);
                out.push(check("policy", Level::Ok, alias, summary(&policy)));
            }
            Err(e) => out.push(failure("policy_unresolved", alias, &e)),
        }
        if let Some(key) = &server.key
            && matches!(parse_reference(key), Ok(None))
            && !key.contains("-----BEGIN")
        {
            out.extend(permissions("key_permissions", &key_path(key, ctx)));
        }
    }

    match daemon {
        Some(status) => out.push(check("daemon_running", Level::Ok, status.pid.to_string(), status.sessions.len().to_string())),
        None => out.push(check("daemon_stopped", Level::Ok, "", "")),
    }
    out
}

fn failure(id: &'static str, subject: &str, error: &Error) -> Check {
    check(id, Level::Fail, subject, format!("{}: {error}", error.code()))
}

fn summary(policy: &crate::policy::Policy) -> String {
    format!(
        "readonly={} allow_commands={} deny_commands={} allow_paths={} deny_paths={}",
        policy.readonly,
        policy.allow_commands.len(),
        policy.deny_commands.len(),
        policy.allow_paths.len(),
        policy.deny_paths.len()
    )
}

fn key_path(spec: &str, ctx: &Context) -> PathBuf {
    let rest = spec.strip_prefix("~/").or_else(|| spec.strip_prefix("~\\"));
    let path = match (rest, &ctx.home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(spec),
    };
    if path.is_absolute() { path } else { ctx.cwd.join(path) }
}

async fn bitwarden(ctx: &Context) -> Vec<Check> {
    let secrets = Secrets::new(&ctx.settings, ctx.env.clone(), ctx.home.clone());
    let status = match secrets.bw_status().await {
        Ok(status) => status,
        Err(e) => return vec![failure("bw_unavailable", "bw", &e)],
    };
    let mut out = vec![check("bw_found", Level::Ok, "bw", status.version.clone())];
    if let Some(expected) = &ctx.settings.bw_server
        && !status.on_server(expected)
    {
        out.push(check("bw_wrong_server", Level::Fail, "bw", format!("connected to {}, expected {expected}", status.server_url)));
    } else {
        out.push(check("bw_server", Level::Ok, "bw", status.server_url.clone()));
    }
    match status.state.as_str() {
        "unlocked" => out.push(check("bw_unlocked", Level::Ok, "bw", "")),
        "locked" => out.push(check("bw_locked", Level::Fail, "bw", "")),
        "unauthenticated" => out.push(check("bw_not_logged_in", Level::Fail, "bw", "")),
        other => out.push(check("bw_state_unknown", Level::Warn, "bw", other)),
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
enum Exposure {
    Tracked,
    NotIgnored,
}

fn git(dir: &Path, args: &[&str], file: &Path) -> Option<i32> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .arg("--")
        .arg(file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()
        .and_then(|s| s.code())
}

/// `None` when git is absent or the file is outside a repository.
fn git_exposure(file: &Path) -> Option<Exposure> {
    let dir = file.parent()?;
    if git(dir, &["check-ignore", "-q"], file)? != 1 {
        return None;
    }
    if git(dir, &["ls-files", "--error-unmatch"], file)? == 0 { Some(Exposure::Tracked) } else { Some(Exposure::NotIgnored) }
}

#[cfg(unix)]
fn permissions(id: &'static str, path: &Path) -> Option<Check> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then(|| check(id, Level::Warn, path.display().to_string(), format!("mode {mode:o}; chmod 600")))
}

/// A heuristic: it reads `icacls` output and looks for the well-known broad principals by their
/// English names, so a localised Windows can slip through.
#[cfg(windows)]
fn permissions(id: &'static str, path: &Path) -> Option<Check> {
    const BROAD: [&str; 4] = ["everyone", "builtin\\users", "nt authority\\authenticated users", "nt authority\\interactive"];
    let output = std::process::Command::new("icacls").arg(path).stdin(std::process::Stdio::null()).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
    let found: Vec<&str> = BROAD.iter().copied().filter(|name| text.contains(&format!("{name}:"))).collect();
    (!found.is_empty()).then(|| check(id, Level::Warn, path.display().to_string(), format!("readable by {}", found.join(", "))))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn group_or_world_access_is_flagged() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "x").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(permissions("env_permissions", &path).unwrap().level, Level::Warn);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(permissions("env_permissions", &path).is_none());
    }
}
