#![allow(dead_code)]

pub mod sftp;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use russh::keys::ssh_key::{self, Algorithm, LineEnding, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, Sig};
use secrecy::SecretString;
use sshhh::config::AuthMethod;
use sshhh::creds::Credentials;
use sshhh::session::{ConnectOptions, HostKeyPolicy};
use sshhh::spec::{Context, ServerSpec};
use tokio::net::TcpListener;

pub const USER: &str = "deploy";
pub const PASSWORD: &str = "correct horse";
pub const SUDO_PASSWORD: &str = "sudo secret";
pub const ROOT_PASSWORD: &str = "root secret";

#[derive(Clone)]
pub struct Behaviour {
    pub authorized_key: Option<PublicKey>,
    pub allow_password: bool,
    pub kbdint: bool,
    pub sudo_nopasswd: bool,
    pub su_prompts: bool,
}

impl Default for Behaviour {
    fn default() -> Self {
        Behaviour { authorized_key: None, allow_password: true, kbdint: false, sudo_nopasswd: false, su_prompts: true }
    }
}

pub struct Fixture {
    pub addr: SocketAddr,
    pub host_key: PublicKey,
    _dir: tempfile::TempDir,
    pub known_hosts: std::path::PathBuf,
    pub fs: sftp::SftpFs,
}

pub fn new_key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
}

pub fn key_text(key: &PrivateKey) -> String {
    key.to_openssh(LineEnding::LF).unwrap().to_string()
}

pub async fn start(behaviour: Behaviour) -> Fixture {
    start_with_host_key(behaviour, new_key()).await
}

pub async fn start_with_host_key(behaviour: Behaviour, host_key: PrivateKey) -> Fixture {
    let config = Arc::new(server::Config {
        auth_rejection_time: Duration::from_millis(1),
        auth_rejection_time_initial: Some(Duration::from_millis(1)),
        keys: vec![host_key.clone()],
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    let fs = sftp::SftpFs::new(dir.path().join("remote"));
    let served = fs.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else { return };
            let config = config.clone();
            let handler = Conn { behaviour: behaviour.clone(), jobs: HashMap::new(), ptys: HashSet::new(), channels: HashMap::new(), fs: served.clone() };
            tokio::spawn(async move {
                if let Ok(session) = server::run_stream(config, socket, handler).await {
                    let _ = session.await;
                }
            });
        }
    });
    Fixture { addr, host_key: host_key.public_key().clone(), _dir: dir, known_hosts, fs }
}

impl Fixture {
    pub fn creds(&self) -> Credentials {
        Credentials {
            alias: "test".into(),
            host: "127.0.0.1".into(),
            port: self.addr.port(),
            user: USER.into(),
            auth: vec![AuthMethod::Password],
            pass: Some(SecretString::from(PASSWORD)),
            key: None,
            key_pass: None,
            cert: None,
        }
    }

    pub fn spec(&self) -> ServerSpec {
        ServerSpec {
            alias: "test".into(),
            host: Some("127.0.0.1".into()),
            port: Some(self.addr.port()),
            user: Some(USER.into()),
            pass: Some(PASSWORD.into()),
            key: None,
            key_pass: None,
            root_user: None,
            root_pass: None,
            sudo_pass: Some(SUDO_PASSWORD.into()),
            escalate: None,
            auth: None,
            vault: None,
            policy: Default::default(),
        }
    }

    pub fn context(&self, home: &std::path::Path) -> Context {
        Context {
            settings: Default::default(),
            env: Default::default(),
            home: Some(home.to_path_buf()),
            cwd: home.to_path_buf(),
            known_hosts: self.known_hosts.clone(),
            accept_new: true,
            connect_timeout_secs: 10,
            audit_commands: false,
        }
    }

    pub fn opts(&self, policy: HostKeyPolicy) -> ConnectOptions {
        ConnectOptions {
            known_hosts: self.known_hosts.clone(),
            policy,
            connect_timeout: Duration::from_secs(10),
        }
    }
}

enum Job {
    Cat(Vec<u8>),
    SudoCat { buf: Vec<u8>, expect: &'static str, rest: String },
    SuPrompt { buf: Vec<u8>, rest: String },
}

struct Conn {
    behaviour: Behaviour,
    jobs: HashMap<ChannelId, Job>,
    ptys: HashSet<ChannelId>,
    channels: HashMap<ChannelId, Channel<Msg>>,
    fs: sftp::SftpFs,
}

fn unquote(quoted: &str) -> String {
    match quoted.strip_prefix('\'').and_then(|q| q.strip_suffix('\'')) {
        Some(inner) => inner.replace(r"'\''", "'"),
        None => quoted.to_string(),
    }
}

fn finish(session: &mut Session, channel: ChannelId, rc: u32) {
    let _ = session.exit_status_request(channel, rc);
    let _ = session.eof(channel);
    let _ = session.close(channel);
}

impl server::Handler for Conn {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let ok = self.behaviour.allow_password && user == USER && password == PASSWORD;
        Ok(if ok { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_publickey(&mut self, user: &str, key: &ssh_key::PublicKey) -> Result<Auth, Self::Error> {
        let ok = user == USER && self.behaviour.authorized_key.as_ref() == Some(key);
        Ok(if ok { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        if !self.behaviour.kbdint || user != USER {
            return Ok(Auth::reject());
        }
        match response {
            None => Ok(Auth::Partial {
                name: "".into(),
                instructions: "".into(),
                prompts: vec![("Password: ".into(), false)].into(),
            }),
            Some(mut answers) => {
                let ok = answers.next().is_some_and(|a| a.as_ref() == PASSWORD.as_bytes());
                Ok(if ok { Auth::Accept } else { Auth::reject() })
            }
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(&mut self, channel: ChannelId, name: &str, session: &mut Session) -> Result<(), Self::Error> {
        match self.channels.remove(&channel) {
            Some(open) if name == "sftp" => {
                session.channel_success(channel)?;
                russh_sftp::server::run(open.into_stream(), sftp::SftpHandler::new(self.fs.clone())).await;
            }
            _ => session.channel_failure(channel)?,
        }
        Ok(())
    }

    async fn exec_request(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).to_string();
        session.channel_success(channel)?;
        match command.as_str() {
            "cat" => {
                self.jobs.insert(channel, Job::Cat(Vec::new()));
            }
            "sleep" => {}
            "dropconn" => {
                session.disconnect(russh::Disconnect::ByApplication, "bye", "en")?;
            }
            "dropsoon" => {
                session.data(channel, b"ok\n".to_vec())?;
                finish(session, channel, 0);
                session.disconnect(russh::Disconnect::ByApplication, "bye", "en")?;
            }
            "kill" => {
                session.exit_signal_request(channel, Sig::KILL, false, "", "")?;
                let _ = session.eof(channel);
                let _ = session.close(channel);
            }
            "noexit" => {
                let _ = session.eof(channel);
                let _ = session.close(channel);
            }
            "both" => {
                session.data(channel, b"out\n".to_vec())?;
                session.extended_data(channel, 1, b"err\n".to_vec())?;
                finish(session, channel, 7);
            }
            "bigout" => {
                let chunk = vec![b'x'; 16 * 1024];
                for _ in 0..64 {
                    session.data(channel, chunk.clone())?;
                }
                finish(session, channel, 0);
            }
            c if c.starts_with("echo ") => {
                session.data(channel, format!("{}\n", &c[5..]).into_bytes())?;
                finish(session, channel, 0);
            }
            c if c.starts_with("exit ") => {
                finish(session, channel, c[5..].trim().parse().unwrap_or(2));
            }
            c if c.starts_with("err ") => {
                session.extended_data(channel, 1, format!("{}\n", &c[4..]).into_bytes())?;
                finish(session, channel, 1);
            }
            "env LC_ALL=C sudo -n true" => {
                if !self.behaviour.sudo_nopasswd {
                    session.extended_data(channel, 1, b"sudo: a password is required\n".to_vec())?;
                }
                finish(session, channel, u32::from(!self.behaviour.sudo_nopasswd));
            }
            c if c.starts_with("env LC_ALL=C sudo -n sh -c ") => {
                if self.behaviour.sudo_nopasswd {
                    let rest = unquote(&c["env LC_ALL=C sudo -n sh -c ".len()..]);
                    session.data(channel, format!("ran as root: {rest}\n").into_bytes())?;
                    finish(session, channel, 0);
                } else {
                    session.extended_data(channel, 1, b"sudo: a password is required\n".to_vec())?;
                    finish(session, channel, 1);
                }
            }
            c if c.starts_with("env LC_ALL=C sudo -S -p '' sh -c ") => {
                let rest = unquote(&c["env LC_ALL=C sudo -S -p '' sh -c ".len()..]);
                self.jobs.insert(channel, Job::SudoCat { buf: Vec::new(), expect: SUDO_PASSWORD, rest });
            }
            c if c.starts_with("env LC_ALL=C su -l root -c ") => {
                if !self.ptys.contains(&channel) {
                    session.extended_data(channel, 1, b"su: must be run from a terminal\n".to_vec())?;
                    finish(session, channel, 1);
                } else if self.behaviour.su_prompts {
                    let rest = unquote(&c["env LC_ALL=C su -l root -c ".len()..]);
                    session.data(channel, b"Password: ".to_vec())?;
                    self.jobs.insert(channel, Job::SuPrompt { buf: Vec::new(), rest });
                }
            }
            _ => {
                session.extended_data(channel, 1, b"unknown command\n".to_vec())?;
                finish(session, channel, 127);
            }
        }
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.ptys.insert(channel);
        session.channel_success(channel)?;
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        match self.jobs.get_mut(&channel) {
            Some(Job::Cat(buf)) | Some(Job::SudoCat { buf, .. }) => buf.extend_from_slice(data),
            Some(Job::SuPrompt { buf, .. }) => {
                buf.extend_from_slice(data);
                if buf.contains(&b'\n') {
                    let Some(Job::SuPrompt { buf, rest }) = self.jobs.remove(&channel) else { return Ok(()) };
                    let given = String::from_utf8_lossy(&buf);
                    if given.trim_end() == ROOT_PASSWORD {
                        session.data(channel, format!("\r\nran as root: {rest}\r\n").into_bytes())?;
                        finish(session, channel, 0);
                    } else {
                        session.data(channel, b"\r\nsu: Authentication failure\r\n".to_vec())?;
                        finish(session, channel, 1);
                    }
                }
            }
            None => {}
        }
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        match self.jobs.remove(&channel) {
            Some(Job::Cat(buf)) => {
                session.data(channel, buf)?;
                finish(session, channel, 0);
            }
            Some(Job::SudoCat { buf, expect, rest }) => {
                let text = String::from_utf8_lossy(&buf).to_string();
                let (given, piped) = text.split_once('\n').unwrap_or((text.as_str(), ""));
                if given == expect {
                    session.data(channel, format!("ran as root: {rest}\n{piped}").into_bytes())?;
                    finish(session, channel, 0);
                } else {
                    session.extended_data(channel, 1, b"Sorry, try again.\nsudo: 1 incorrect password attempt\n".to_vec())?;
                    finish(session, channel, 1);
                }
            }
            Some(Job::SuPrompt { .. }) | None => {}
        }
        Ok(())
    }
}
