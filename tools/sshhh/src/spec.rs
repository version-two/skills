use std::collections::BTreeMap;
use std::path::PathBuf;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{AuthMethod, Escalate, Server, Settings};
use crate::secrets::{Target, parse_reference};

/// A server as written in the `.env`, with references unresolved. It travels to the daemon, so
/// it carries literal secrets and must never be logged.
#[derive(Clone, Serialize, Deserialize)]
pub struct ServerSpec {
    pub alias: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub pass: Option<String>,
    pub key: Option<String>,
    pub key_pass: Option<String>,
    pub root_user: Option<String>,
    pub root_pass: Option<String>,
    pub sudo_pass: Option<String>,
    pub escalate: Option<Escalate>,
    pub auth: Option<Vec<AuthMethod>>,
    pub vault: Option<String>,
}

fn reveal(secret: &Option<SecretString>) -> Option<String> {
    secret.as_ref().map(|s| s.expose_secret().to_owned())
}

impl ServerSpec {
    pub fn from_server(server: &Server) -> ServerSpec {
        ServerSpec {
            alias: server.alias.clone(),
            host: server.host.clone(),
            port: server.port,
            user: server.user.clone(),
            pass: reveal(&server.pass),
            key: server.key.clone(),
            key_pass: reveal(&server.key_pass),
            root_user: server.root_user.clone(),
            root_pass: reveal(&server.root_pass),
            sudo_pass: reveal(&server.sudo_pass),
            escalate: server.escalate,
            auth: server.auth.clone(),
            vault: server.vault.clone(),
        }
    }

    pub fn to_server(&self) -> Server {
        let secret = |v: &Option<String>| v.clone().map(SecretString::from);
        Server {
            alias: self.alias.clone(),
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone(),
            pass: secret(&self.pass),
            key: self.key.clone(),
            key_pass: secret(&self.key_pass),
            root_user: self.root_user.clone(),
            root_pass: secret(&self.root_pass),
            sudo_pass: secret(&self.sudo_pass),
            escalate: self.escalate,
            auth: self.auth.clone(),
            vault: self.vault.clone(),
        }
    }

    fn values(&self) -> [&Option<String>; 9] {
        [&self.host, &self.user, &self.pass, &self.key, &self.key_pass, &self.root_user, &self.root_pass, &self.sudo_pass, &self.vault]
    }

    /// Names of the environment variables the spec reads through `env://`.
    pub fn env_references(&self) -> Vec<String> {
        self.values()
            .into_iter()
            .flatten()
            .filter_map(|v| parse_reference(v).ok().flatten())
            .filter_map(|r| match r.target {
                Target::Env(name) => Some(name),
                _ => None,
            })
            .collect()
    }

    /// Values that are literal secrets, known without resolving anything.
    pub fn literal_secrets(&self) -> Vec<&str> {
        [&self.pass, &self.key_pass, &self.root_pass, &self.sudo_pass]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .filter(|v| matches!(parse_reference(v), Ok(None)))
            .collect()
    }
}

/// Everything about the caller's surroundings that decides how a spec resolves and connects.
#[derive(Clone, Serialize, Deserialize)]
pub struct Context {
    pub settings: Settings,
    pub env: BTreeMap<String, String>,
    pub home: Option<PathBuf>,
    pub cwd: PathBuf,
    pub known_hosts: PathBuf,
    pub accept_new: bool,
    pub connect_timeout_secs: u64,
    pub audit_commands: bool,
}

impl Context {
    pub fn session_key(&self, spec: &ServerSpec) -> String {
        let mut hash = Sha256::new();
        let mut part = |text: &str| {
            hash.update((text.len() as u64).to_be_bytes());
            hash.update(text.as_bytes());
        };
        part(&serde_json::to_string(spec).unwrap_or_default());
        part(&serde_json::to_string(&self.settings).unwrap_or_default());
        part(&self.known_hosts.display().to_string());
        part(if self.accept_new { "accept-new" } else { "strict" });
        part(&self.cwd.display().to_string());
        for name in spec.env_references().into_iter().chain(["BW_SESSION".to_string()]) {
            part(&name);
            part(self.env.get(&name).map(String::as_str).unwrap_or(""));
        }
        hex(&hash.finalize())
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(pass: &str) -> ServerSpec {
        ServerSpec {
            alias: "zeus".into(),
            host: Some("h".into()),
            port: None,
            user: None,
            pass: Some(pass.into()),
            key: None,
            key_pass: None,
            root_user: None,
            root_pass: Some("env://ROOTPW".into()),
            sudo_pass: None,
            escalate: None,
            auth: None,
            vault: None,
        }
    }

    fn context() -> Context {
        Context {
            settings: Settings::default(),
            env: BTreeMap::new(),
            home: None,
            cwd: PathBuf::from("/work"),
            known_hosts: PathBuf::from("/kh"),
            accept_new: false,
            connect_timeout_secs: 15,
            audit_commands: false,
        }
    }

    #[test]
    fn session_key_changes_with_the_spec_and_the_referenced_environment() {
        let c = context();
        assert_eq!(c.session_key(&spec("a")), c.session_key(&spec("a")));
        assert_ne!(c.session_key(&spec("a")), c.session_key(&spec("b")));
        let mut other = context();
        other.env.insert("ROOTPW".into(), "x".into());
        assert_ne!(c.session_key(&spec("a")), other.session_key(&spec("a")));
        other.env.insert("UNRELATED".into(), "y".into());
        let mut again = context();
        again.env.insert("ROOTPW".into(), "x".into());
        assert_eq!(other.session_key(&spec("a")), again.session_key(&spec("a")));
    }

    #[test]
    fn only_literals_count_as_known_secrets_and_references_name_their_variables() {
        let s = spec("literal");
        assert_eq!(s.literal_secrets(), ["literal"]);
        assert_eq!(s.env_references(), ["ROOTPW"]);
    }
}
