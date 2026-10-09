use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use crate::config::{Escalate, Server};
use crate::error::Error;
use crate::resolve::EscalationSecrets;
use crate::session::shell_quote;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RootMode {
    Auto,
    Sudo,
    Su,
}

pub enum Mechanism {
    Direct,
    Sudo { password: Option<SecretString> },
    Su { user: String, password: SecretString },
}

impl Mechanism {
    pub fn name(&self) -> &'static str {
        match self {
            Mechanism::Direct => "none",
            Mechanism::Sudo { .. } => "sudo",
            Mechanism::Su { .. } => "su",
        }
    }
}

fn copy(secret: &SecretString) -> SecretString {
    SecretString::from(secret.expose_secret().to_owned())
}

/// `--sudo` / `--su` win over `ESCALATE`; without either, the secrets that exist decide:
/// SUDO_PASS gives sudo, else ROOT_PASS gives su, else passwordless sudo. A root login needs nothing.
pub fn choose(server: &Server, login_user: &str, mode: RootMode, secrets: &EscalationSecrets) -> Result<Mechanism, Error> {
    let forced = match mode {
        RootMode::Auto => server.escalate,
        RootMode::Sudo => Some(Escalate::Sudo),
        RootMode::Su => Some(Escalate::Su),
    };
    let sudo = || Mechanism::Sudo { password: secrets.sudo_pass.as_ref().map(copy) };
    let su = || match &secrets.root_pass {
        Some(password) => Ok(Mechanism::Su { user: secrets.root_user.clone(), password: copy(password) }),
        None => Err(Error::EscalationFailed { mechanism: "su", reason: format!("server '{}' has no ROOT_PASS", server.alias) }),
    };
    match forced {
        Some(Escalate::None) if login_user == "root" => Ok(Mechanism::Direct),
        Some(Escalate::None) => Err(Error::EscalationFailed {
            mechanism: "none",
            reason: format!("server '{}' has ESCALATE=none and the login user is not root", server.alias),
        }),
        Some(Escalate::Sudo) => Ok(sudo()),
        Some(Escalate::Su) => su(),
        None if login_user == "root" => Ok(Mechanism::Direct),
        None if secrets.sudo_pass.is_some() => Ok(sudo()),
        None if secrets.root_pass.is_some() => su(),
        None => Ok(sudo()),
    }
}

/// `LC_ALL=C` keeps the failure messages in English so they can be recognised.
pub fn sudo_command(command: &str, with_password: bool) -> String {
    let flags = if with_password { "-S -p ''" } else { "-n" };
    format!("env LC_ALL=C sudo {flags} sh -c {}", shell_quote(command))
}

pub const SUDO_PROBE: &str = "env LC_ALL=C sudo -n true";

pub fn su_command(user: &str, command: &str) -> String {
    format!("env LC_ALL=C su -l {} -c {}", shell_quote(user), shell_quote(command))
}

pub fn sudo_failure(rc: i32, stderr_tail: &str) -> Option<&'static str> {
    let text = stderr_tail.to_ascii_lowercase();
    if text.contains("incorrect password attempt") {
        Some("the sudo password was rejected")
    } else if text.contains("is not in the sudoers file") || text.contains("is not allowed to execute") {
        Some("the login user is not allowed to use sudo for this")
    } else if rc == 1 && text.contains("a password is required") {
        Some("sudo needs a password; set SUDO_PASS")
    } else {
        None
    }
}

pub fn su_failure(output_tail: &str) -> Option<&'static str> {
    let text = output_tail.to_ascii_lowercase();
    if text.contains("authentication failure") || text.contains("incorrect password") {
        Some("the root password was rejected")
    } else if text.contains("must be run from a terminal") {
        Some("su refused to run without a terminal")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets(sudo: bool, root: bool) -> EscalationSecrets {
        EscalationSecrets {
            root_user: "root".into(),
            root_pass: root.then(|| SecretString::from("r")),
            sudo_pass: sudo.then(|| SecretString::from("s")),
        }
    }

    fn server(escalate: Option<Escalate>) -> Server {
        Server {
            alias: "z".into(),
            host: Some("h".into()),
            port: None,
            user: None,
            pass: None,
            key: None,
            key_pass: None,
            root_user: None,
            root_pass: None,
            sudo_pass: None,
            escalate,
            auth: None,
            vault: None,
            policy: Default::default(),
        }
    }

    fn name(m: Result<Mechanism, Error>) -> &'static str {
        m.map(|m| m.name()).unwrap_or("error")
    }

    #[test]
    fn the_secrets_that_exist_pick_the_mechanism() {
        let s = server(None);
        assert_eq!(name(choose(&s, "root", RootMode::Auto, &secrets(true, true))), "none");
        assert_eq!(name(choose(&s, "deploy", RootMode::Auto, &secrets(true, true))), "sudo");
        assert_eq!(name(choose(&s, "deploy", RootMode::Auto, &secrets(false, true))), "su");
        assert_eq!(name(choose(&s, "deploy", RootMode::Auto, &secrets(false, false))), "sudo");
    }

    #[test]
    fn flags_and_the_escalate_key_override_inference() {
        let s = server(None);
        assert_eq!(name(choose(&s, "deploy", RootMode::Sudo, &secrets(false, true))), "sudo");
        assert_eq!(name(choose(&s, "deploy", RootMode::Su, &secrets(true, true))), "su");
        assert_eq!(name(choose(&s, "deploy", RootMode::Su, &secrets(true, false))), "error");
        assert_eq!(name(choose(&server(Some(Escalate::Su)), "deploy", RootMode::Auto, &secrets(true, true))), "su");
        assert_eq!(name(choose(&server(Some(Escalate::None)), "deploy", RootMode::Auto, &secrets(true, true))), "error");
        assert_eq!(name(choose(&server(Some(Escalate::None)), "root", RootMode::Auto, &secrets(true, true))), "none");
    }

    #[test]
    fn commands_never_contain_a_password() {
        assert_eq!(sudo_command("systemctl restart x", true), "env LC_ALL=C sudo -S -p '' sh -c 'systemctl restart x'");
        assert_eq!(sudo_command("id", false), "env LC_ALL=C sudo -n sh -c id");
        assert_eq!(su_command("root", "it's"), r"env LC_ALL=C su -l root -c 'it'\''s'");
    }

    #[test]
    fn failures_are_recognised_from_the_english_messages() {
        assert!(sudo_failure(1, "Sorry, try again.\nsudo: 3 incorrect password attempts").is_some());
        assert!(sudo_failure(1, "sudo: a password is required").is_some());
        assert!(sudo_failure(2, "sudo: a password is required").is_none());
        assert!(sudo_failure(0, "all fine").is_none());
        assert!(su_failure("su: Authentication failure").is_some());
        assert!(su_failure("hello").is_none());
    }
}
