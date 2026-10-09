#![allow(dead_code)]

use std::collections::HashMap;
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
}

impl Default for Behaviour {
    fn default() -> Self {
        Behaviour { authorized_key: None, allow_password: true, kbdint: false }
    }
}

pub struct Fixture {
    pub addr: SocketAddr,
    pub host_key: PublicKey,
    _dir: tempfile::TempDir,
    pub known_hosts: std::path::PathBuf,
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
    let mut config = server::Config::default();
    config.auth_rejection_time = Duration::from_millis(1);
    config.auth_rejection_time_initial = Some(Duration::from_millis(1));
    config.keys.push(host_key.clone());
    let config = Arc::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else { return };
            let config = config.clone();
            let handler = Conn { behaviour: behaviour.clone(), jobs: HashMap::new() };
            tokio::spawn(async move {
                if let Ok(session) = server::run_stream(config, socket, handler).await {
                    let _ = session.await;
                }
            });
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    Fixture { addr, host_key: host_key.public_key().clone(), _dir: dir, known_hosts }
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
}

struct Conn {
    behaviour: Behaviour,
    jobs: HashMap<ChannelId, Job>,
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
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
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
            c if c.starts_with("sudo -S -p '' ") => {
                let rest = c["sudo -S -p '' ".len()..].to_string();
                self.jobs.insert(channel, Job::SudoCat { buf: Vec::new(), expect: SUDO_PASSWORD, rest });
            }
            _ => {
                session.extended_data(channel, 1, b"unknown command\n".to_vec())?;
                finish(session, channel, 127);
            }
        }
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], _session: &mut Session) -> Result<(), Self::Error> {
        match self.jobs.get_mut(&channel) {
            Some(Job::Cat(buf)) => buf.extend_from_slice(data),
            Some(Job::SudoCat { buf, .. }) => buf.extend_from_slice(data),
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
                let (given, _) = text.split_once('\n').unwrap_or((text.as_str(), ""));
                if given == expect {
                    session.data(channel, format!("ran as root: {rest}\n").into_bytes())?;
                    finish(session, channel, 0);
                } else {
                    session.extended_data(channel, 1, b"Sorry, try again.\n".to_vec())?;
                    finish(session, channel, 1);
                }
            }
            None => {}
        }
        Ok(())
    }
}
