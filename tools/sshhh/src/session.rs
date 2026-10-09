use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect, Sig};
use secrecy::ExposeSecret;
use tokio::io::AsyncWriteExt;

use crate::config::AuthMethod;
use crate::creds::Credentials;
use crate::error::Error;

const KBDINT_ROUNDS: usize = 5;

#[cfg(windows)]
const AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyPolicy {
    Strict,
    AcceptNew,
}

pub struct ConnectOptions {
    pub known_hosts: PathBuf,
    pub policy: HostKeyPolicy,
    pub connect_timeout: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutcome {
    pub rc: i32,
    pub signal: Option<String>,
}

enum HostKeyFault {
    Unknown { fingerprint: String },
    Changed { fingerprint: String, line: usize },
    Unusable(String),
}

struct Client {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    policy: HostKeyPolicy,
    fault: Arc<Mutex<Option<HostKeyFault>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, presented: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let fault = match presented {
            PublicKeyOrCertificate::PublicKey { key, .. } => {
                let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
                match keys::known_hosts::check_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
                    Ok(true) => return Ok(true),
                    Ok(false) if self.policy == HostKeyPolicy::AcceptNew => {
                        match keys::known_hosts::learn_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
                            Ok(()) => return Ok(true),
                            Err(e) => HostKeyFault::Unusable(e.to_string()),
                        }
                    }
                    Ok(false) => HostKeyFault::Unknown { fingerprint },
                    Err(keys::Error::KeyChanged { line }) => HostKeyFault::Changed { fingerprint, line },
                    Err(e) => HostKeyFault::Unusable(e.to_string()),
                }
            }
            PublicKeyOrCertificate::Certificate(_) => HostKeyFault::Unusable("host certificates are not supported".into()),
        };
        *self.fault.lock().unwrap_or_else(PoisonError::into_inner) = Some(fault);
        Ok(false)
    }
}

pub struct Session {
    handle: client::Handle<Client>,
    host: String,
}

fn client_config() -> client::Config {
    client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        nodelay: true,
        window_size: 8 * 1024 * 1024,
        ..Default::default()
    }
}

impl Session {
    pub async fn connect(creds: &Credentials, opts: &ConnectOptions) -> Result<Session, Error> {
        if creds.auth.is_empty() {
            return Err(Error::NoCredentials { alias: creds.alias.clone() });
        }
        let fault = Arc::new(Mutex::new(None));
        let handler = Client {
            host: creds.host.clone(),
            port: creds.port,
            known_hosts: opts.known_hosts.clone(),
            policy: opts.policy,
            fault: fault.clone(),
        };
        let attempt = async {
            let mut handle = client::connect(Arc::new(client_config()), (creds.host.as_str(), creds.port), handler)
                .await
                .map_err(|e| connect_error(e, &fault, creds))?;
            authenticate(&mut handle, creds).await?;
            Ok::<_, Error>(handle)
        };
        let handle = tokio::time::timeout(opts.connect_timeout, attempt).await.map_err(|_| Error::ConnectTimeout {
            host: creds.host.clone(),
            port: creds.port,
            secs: opts.connect_timeout.as_secs(),
        })??;
        Ok(Session { handle, host: creds.host.clone() })
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    pub async fn close(&self) {
        let _ = self.handle.disconnect(Disconnect::ByApplication, "", "en").await;
    }

    pub async fn exec(
        &self,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        sink: &mut (dyn FnMut(Stream, &[u8]) + Send),
    ) -> Result<ExecOutcome, Error> {
        let lost = |reason: String| Error::ConnectionLost { host: self.host.clone(), reason };
        let mut channel = self.handle.channel_open_session().await.map_err(|e| lost(e.to_string()))?;
        channel.exec(true, command).await.map_err(|e| lost(e.to_string()))?;

        match stdin.filter(|bytes| !bytes.is_empty()) {
            Some(bytes) => {
                let mut writer = channel.make_writer();
                tokio::spawn(async move {
                    if writer.write_all(&bytes).await.is_ok() {
                        let _ = writer.shutdown().await;
                    }
                });
            }
            None => channel.eof().await.map_err(|e| lost(e.to_string()))?,
        }

        let mut status = None;
        let mut signal = None;
        let run = async {
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) => sink(Stream::Stdout, &data),
                    Some(ChannelMsg::ExtendedData { data, .. }) => sink(Stream::Stderr, &data),
                    Some(ChannelMsg::ExitStatus { exit_status }) => status = Some(exit_status),
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => signal = Some(signal_name),
                    Some(ChannelMsg::Failure) => return Err(Error::Refused("exec request denied".into())),
                    Some(ChannelMsg::Close) | None => return Ok(()),
                    Some(_) => {}
                }
            }
        };
        if tokio::time::timeout(timeout, run).await.is_err() {
            let _ = channel.close().await;
            return Err(Error::CommandTimeout { secs: timeout.as_secs() });
        }
        run_outcome(status, signal).ok_or_else(|| lost("channel closed without an exit status".into()))
    }
}

fn run_outcome(status: Option<u32>, signal: Option<Sig>) -> Option<ExecOutcome> {
    if let Some(sig) = signal {
        let (name, number) = match &sig {
            Sig::HUP => ("HUP", 1),
            Sig::INT => ("INT", 2),
            Sig::QUIT => ("QUIT", 3),
            Sig::ILL => ("ILL", 4),
            Sig::ABRT => ("ABRT", 6),
            Sig::FPE => ("FPE", 8),
            Sig::KILL => ("KILL", 9),
            Sig::USR1 => ("USR1", 10),
            Sig::SEGV => ("SEGV", 11),
            Sig::PIPE => ("PIPE", 13),
            Sig::ALRM => ("ALRM", 14),
            Sig::TERM => ("TERM", 15),
            Sig::Custom(name) => (name.as_str(), 127),
        };
        return Some(ExecOutcome { rc: 128 + number, signal: Some(name.to_string()) });
    }
    status.map(|s| ExecOutcome { rc: i32::try_from(s).unwrap_or(i32::MAX), signal: None })
}

fn connect_error(e: russh::Error, fault: &Mutex<Option<HostKeyFault>>, creds: &Credentials) -> Error {
    let (host, port) = (creds.host.clone(), creds.port);
    match fault.lock().unwrap_or_else(PoisonError::into_inner).take() {
        Some(HostKeyFault::Unknown { fingerprint }) => Error::HostKeyUnknown { host, port, fingerprint },
        Some(HostKeyFault::Changed { fingerprint, line }) => Error::HostKeyChanged { host, port, fingerprint, line },
        Some(HostKeyFault::Unusable(reason)) => Error::HostKeyUnusable { host, port, reason },
        None => Error::Connect { host, port, reason: e.to_string() },
    }
}

fn auth_name(method: AuthMethod) -> &'static str {
    match method {
        AuthMethod::Key => "key",
        AuthMethod::Password => "password",
        AuthMethod::Kbdint => "kbdint",
        AuthMethod::Agent => "agent",
    }
}

async fn authenticate(handle: &mut client::Handle<Client>, creds: &Credentials) -> Result<(), Error> {
    let lost = |e: russh::Error| Error::ConnectionLost { host: creds.host.clone(), reason: e.to_string() };
    let missing = || Error::NoCredentials { alias: creds.alias.clone() };
    let mut tried = Vec::new();
    let mut server_methods = String::from("unknown");
    for &method in &creds.auth {
        tried.push(auth_name(method));
        let result = match method {
            AuthMethod::Password => {
                let pass = creds.pass.as_ref().ok_or_else(missing)?;
                handle.authenticate_password(creds.user.as_str(), pass.expose_secret()).await.map_err(lost)?
            }
            AuthMethod::Kbdint => {
                let pass = creds.pass.as_ref().ok_or_else(missing)?;
                match keyboard_interactive(handle, &creds.user, pass.expose_secret()).await.map_err(lost)? {
                    KeyboardInteractiveAuthResponse::Success => return Ok(()),
                    KeyboardInteractiveAuthResponse::Failure { remaining_methods, .. } => {
                        server_methods = format!("{remaining_methods:?}");
                        continue;
                    }
                    KeyboardInteractiveAuthResponse::InfoRequest { .. } => continue,
                }
            }
            AuthMethod::Key => {
                let text = creds.key.as_ref().ok_or_else(missing)?;
                let passphrase = creds.key_pass.as_ref().map(|p| p.expose_secret());
                let key = keys::decode_secret_key(text.expose_secret(), passphrase)
                    .map_err(|e| Error::KeyUnusable { reason: e.to_string() })?;
                let key = Arc::new(key);
                match &creds.cert {
                    Some(cert) => {
                        let cert = keys::Certificate::from_openssh(cert)
                            .map_err(|e| Error::KeyUnusable { reason: format!("certificate: {e}") })?;
                        handle.authenticate_openssh_cert(creds.user.as_str(), key, cert).await.map_err(lost)?
                    }
                    None => {
                        let hash = handle.best_supported_rsa_hash().await.map_err(lost)?.flatten();
                        handle
                            .authenticate_publickey(creds.user.as_str(), PrivateKeyWithHashAlg::new(key, hash))
                            .await
                            .map_err(lost)?
                    }
                }
            }
            AuthMethod::Agent => agent_auth(handle, creds).await?,
        };
        match result {
            russh::client::AuthResult::Success => return Ok(()),
            russh::client::AuthResult::Failure { remaining_methods, .. } => {
                server_methods = format!("{remaining_methods:?}");
            }
        }
    }
    Err(Error::AuthFailed { user: creds.user.clone(), tried: tried.join(", "), server_methods })
}

async fn keyboard_interactive(
    handle: &mut client::Handle<Client>,
    user: &str,
    password: &str,
) -> Result<KeyboardInteractiveAuthResponse, russh::Error> {
    let mut response = handle.authenticate_keyboard_interactive_start(user, None).await?;
    for _ in 0..KBDINT_ROUNDS {
        match response {
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                response = handle.authenticate_keyboard_interactive_respond(answers).await?;
            }
            done => return Ok(done),
        }
    }
    Ok(response)
}

async fn agent_auth(
    handle: &mut client::Handle<Client>,
    creds: &Credentials,
) -> Result<russh::client::AuthResult, Error> {
    let unavailable = |reason: String| Error::AgentUnavailable { reason };
    #[cfg(windows)]
    let mut agent = keys::agent::client::AgentClient::connect_named_pipe(AGENT_PIPE)
        .await
        .map_err(|e| unavailable(e.to_string()))?
        .dynamic();
    #[cfg(unix)]
    let mut agent =
        keys::agent::client::AgentClient::connect_env().await.map_err(|e| unavailable(e.to_string()))?.dynamic();

    let identities = agent.request_identities().await.map_err(|e| unavailable(e.to_string()))?;
    if identities.is_empty() {
        return Err(unavailable("the agent holds no identities".into()));
    }
    let hash = handle
        .best_supported_rsa_hash()
        .await
        .map_err(|e| Error::ConnectionLost { host: creds.host.clone(), reason: e.to_string() })?
        .flatten();
    let mut last = None;
    for identity in identities {
        let result = handle
            .authenticate_publickey_with(creds.user.as_str(), identity.public_key().into_owned(), hash, &mut agent)
            .await
            .map_err(|e| unavailable(e.to_string()))?;
        if result.success() {
            return Ok(result);
        }
        last = Some(result);
    }
    Ok(last.expect("at least one identity was tried"))
}

pub fn shell_quote(arg: &str) -> String {
    if !arg.is_empty() && arg.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-./=:,@%+".contains(&b)) {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

pub fn command_from_args(args: &[String]) -> String {
    match args {
        [single] => single.clone(),
        many => many.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_is_safe_for_the_remote_shell() {
        assert_eq!(shell_quote("plain-1.2/x"), "plain-1.2/x");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
    }

    #[test]
    fn single_argument_is_a_verbatim_shell_string() {
        assert_eq!(command_from_args(&["ls | wc -l".into()]), "ls | wc -l");
        assert_eq!(command_from_args(&["ls".into(), "my dir".into()]), "ls 'my dir'");
    }

    #[test]
    fn signals_map_to_shell_style_codes() {
        let o = run_outcome(None, Some(Sig::KILL)).unwrap();
        assert_eq!((o.rc, o.signal.as_deref()), (137, Some("KILL")));
        assert_eq!(run_outcome(Some(3), None).unwrap().rc, 3);
        assert!(run_outcome(None, None).is_none());
    }
}
