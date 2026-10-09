use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, Instant};

use russh::Channel;
use russh::client::Msg;
use russh_sftp::client::SftpSession;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OnceCell};

use crate::audit::{AuditLog, Entry};
use crate::config::Server;
use crate::creds::Credentials;
use crate::error::Error;
use crate::escalate::{self, Mechanism, RootMode};
use crate::policy::Policy;
use crate::redact::Redactor;
use crate::resolve::{EscalationSecrets, Resolver};
use crate::session::{ConnectOptions, HostKeyPolicy, Session, Stream};
use crate::spec::{Context, ServerSpec};
use crate::transfer::{self, Listing, Options, TransferReport};

const TAIL: usize = 2048;
const PROMPT_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Serialize, Deserialize)]
pub struct RunRequest {
    pub command: String,
    pub root: Option<RootMode>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    pub alias: String,
    pub rc: i32,
    pub signal: Option<String>,
    pub duration_ms: u64,
    pub escalation: String,
    pub reconnected: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub alias: String,
    pub user: String,
    pub idle_secs: u64,
}

pub type Sink<'a> = &'a mut (dyn FnMut(Stream, &[u8]) + Send);

pub struct Held {
    pub session: Session,
    pub login_user: String,
    pub alias: String,
    pub server: Server,
    /// The `.env` policy plus whatever the vault item adds; both only ever restrict.
    pub policy: Policy,
    resolver: Resolver,
    escalation: OnceCell<EscalationSecrets>,
    passwordless: OnceCell<bool>,
    base_secrets: Vec<String>,
    last_used: StdMutex<Instant>,
}

impl Held {
    fn touch(&self) {
        *self.last_used.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    fn idle(&self) -> Duration {
        self.last_used.lock().unwrap_or_else(PoisonError::into_inner).elapsed()
    }

    pub async fn mechanism(&self, mode: RootMode) -> Result<Mechanism, Error> {
        let secrets = self.escalation.get_or_try_init(|| self.resolver.escalation(&self.server)).await?;
        escalate::choose(&self.server, &self.login_user, mode, secrets)
    }

    pub fn redaction_values(&self) -> Vec<String> {
        let mut values = self.base_secrets.clone();
        if let Some(e) = self.escalation.get() {
            values.extend([&e.root_pass, &e.sudo_pass].into_iter().flatten().map(|s| s.expose_secret().to_owned()));
        }
        values
    }
}

type Slot = Arc<Mutex<Option<Arc<Held>>>>;

pub struct Engine {
    slots: Mutex<HashMap<String, Slot>>,
    prompt_timeout: Duration,
}

impl Default for Engine {
    fn default() -> Self {
        Engine { slots: Mutex::default(), prompt_timeout: PROMPT_TIMEOUT }
    }
}

fn base_secrets(spec: &ServerSpec, creds: &Credentials) -> Vec<String> {
    let mut values: Vec<String> = spec.literal_secrets().into_iter().map(str::to_owned).collect();
    values.extend([&creds.pass, &creds.key_pass].into_iter().flatten().map(|s| s.expose_secret().to_owned()));
    values
}

async fn connect(ctx: &Context, spec: &ServerSpec) -> Result<Held, Error> {
    let server = spec.to_server();
    let resolver = Resolver::new(&ctx.settings, ctx.env.clone(), ctx.home.clone(), ctx.cwd.clone());
    let creds = resolver.credentials(&server).await?;
    let mut policy = spec.policy.clone();
    policy.merge(&resolver.vault_policy(&server).await?);
    let opts = ConnectOptions {
        known_hosts: ctx.known_hosts.clone(),
        policy: if ctx.accept_new { HostKeyPolicy::AcceptNew } else { HostKeyPolicy::Strict },
        connect_timeout: Duration::from_secs(ctx.connect_timeout_secs),
    };
    let session = Session::connect(&creds, &opts).await?;
    Ok(Held {
        session,
        login_user: creds.user.clone(),
        alias: spec.alias.clone(),
        base_secrets: base_secrets(spec, &creds),
        server,
        policy,
        resolver,
        escalation: OnceCell::new(),
        passwordless: OnceCell::new(),
        last_used: StdMutex::new(Instant::now()),
    })
}

fn push_tail(tail: &mut Vec<u8>, bytes: &[u8]) {
    tail.extend_from_slice(bytes);
    if tail.len() > TAIL {
        tail.drain(..tail.len() - TAIL);
    }
}

fn open_audit(ctx: &Context) -> Result<AuditLog, Error> {
    let home = ctx.home.as_deref().ok_or_else(|| Error::Usage("cannot determine the home directory for the audit log".into()))?;
    AuditLog::open(home)
}

impl Engine {
    pub fn new() -> Engine {
        Engine::default()
    }

    pub fn with_prompt_timeout(mut self, timeout: Duration) -> Engine {
        self.prompt_timeout = timeout;
        self
    }

    pub async fn session_count(&self) -> usize {
        let slots: Vec<Slot> = self.slots.lock().await.values().cloned().collect();
        let mut live = 0;
        for slot in slots {
            if slot.lock().await.as_ref().is_some_and(|h| !h.session.is_closed()) {
                live += 1;
            }
        }
        live
    }

    pub async fn sessions(&self) -> Vec<SessionInfo> {
        let slots: Vec<Slot> = self.slots.lock().await.values().cloned().collect();
        let mut out = Vec::new();
        for slot in slots {
            if let Some(held) = slot.lock().await.as_ref().filter(|h| !h.session.is_closed()) {
                out.push(SessionInfo { alias: held.alias.clone(), user: held.login_user.clone(), idle_secs: held.idle().as_secs() });
            }
        }
        out.sort_by(|a, b| a.alias.cmp(&b.alias));
        out
    }

    pub async fn sweep(&self, idle: Duration) -> usize {
        let slots: Vec<(String, Slot)> = self.slots.lock().await.iter().map(|(k, s)| (k.clone(), s.clone())).collect();
        let mut dropped = Vec::new();
        for (key, slot) in slots {
            let mut guard = slot.lock().await;
            let expired = guard.as_ref().is_some_and(|h| h.session.is_closed() || h.idle() > idle);
            if expired {
                if let Some(held) = guard.take() {
                    held.session.close().await;
                }
                dropped.push(key);
            }
        }
        let mut slots = self.slots.lock().await;
        for key in &dropped {
            slots.remove(key);
        }
        dropped.len()
    }

    pub async fn shutdown(&self) {
        let _ = self.sweep(Duration::ZERO).await;
    }

    async fn slot(&self, key: &str) -> Slot {
        self.slots.lock().await.entry(key.to_string()).or_default().clone()
    }

    async fn evict(&self, key: &str, held: &Arc<Held>) {
        let slot = self.slot(key).await;
        let mut guard = slot.lock().await;
        if guard.as_ref().is_some_and(|current| Arc::ptr_eq(current, held)) {
            *guard = None;
        }
    }

    /// Returns the live session for this spec, connecting when there is none. `true` marks a
    /// replacement of a session that had died.
    pub async fn acquire(&self, ctx: &Context, spec: &ServerSpec) -> Result<(Arc<Held>, bool), Error> {
        let slot = self.slot(&ctx.session_key(spec)).await;
        let mut guard = slot.lock().await;
        if let Some(held) = guard.as_ref().filter(|h| !h.session.is_closed()) {
            held.touch();
            return Ok((held.clone(), false));
        }
        let replaced = guard.is_some();
        let held = Arc::new(connect(ctx, spec).await?);
        *guard = Some(held.clone());
        Ok((held, replaced))
    }

    /// A channel that failed to open has carried no command yet, so one reconnect is safe.
    pub async fn open_channel(&self, ctx: &Context, spec: &ServerSpec) -> Result<(Arc<Held>, Channel<Msg>, bool), Error> {
        let (held, mut reconnected) = self.acquire(ctx, spec).await?;
        match held.session.open().await {
            Ok(channel) => Ok((held, channel, reconnected)),
            Err(_) => {
                self.evict(&ctx.session_key(spec), &held).await;
                let (fresh, _) = self.acquire(ctx, spec).await?;
                reconnected = true;
                let channel = fresh.session.open().await?;
                Ok((fresh, channel, reconnected))
            }
        }
    }

    pub async fn run(
        &self,
        ctx: &Context,
        spec: &ServerSpec,
        request: &RunRequest,
        stdin: Option<Vec<u8>>,
        sink: Sink<'_>,
    ) -> Result<RunReport, Error> {
        let started = Instant::now();
        let mut audit = open_audit(ctx)?;
        let outcome = self.run_inner(ctx, spec, request, stdin, sink).await;
        let (alias, user, mechanism) = match &outcome {
            Ok((report, user)) => (report.alias.as_str(), user.as_str(), report.escalation.as_str()),
            Err(_) => (spec.alias.as_str(), spec.user.as_deref().unwrap_or("root"), "unknown"),
        };
        let logged = audit.write(&Entry {
            alias,
            user,
            op: "run",
            mechanism,
            rc: outcome.as_ref().ok().map(|(r, _)| r.rc),
            error: outcome.as_ref().err().map(|e| e.code().to_owned()),
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            subject: &request.command,
            store_subject: ctx.audit_commands,
        });
        let (mut report, _) = outcome?;
        report.warnings.extend(logged.err());
        Ok(report)
    }

    async fn run_inner(
        &self,
        ctx: &Context,
        spec: &ServerSpec,
        request: &RunRequest,
        stdin: Option<Vec<u8>>,
        sink: Sink<'_>,
    ) -> Result<(RunReport, String), Error> {
        let started = Instant::now();
        spec.policy.check_exec(&request.command, request.root.is_some())?;
        if stdin.is_some() {
            spec.policy.check_stdin()?;
        }
        let (held, channel, reconnected) = self.open_channel(ctx, spec).await?;
        held.policy.check_exec(&request.command, request.root.is_some())?;
        if stdin.is_some() {
            held.policy.check_stdin()?;
        }
        let mechanism = match request.root {
            Some(mode) => held.mechanism(mode).await?,
            None => Mechanism::Direct,
        };
        let timeout = Duration::from_secs(request.timeout_secs);

        let values = held.redaction_values();
        let mut redact = [Redactor::new(values.iter().map(String::as_str)), Redactor::new(values.iter().map(String::as_str))];
        let mut tails = [Vec::new(), Vec::new()];
        let mut forward = |stream: Stream, bytes: &[u8]| {
            let index = usize::from(stream == Stream::Stderr);
            push_tail(&mut tails[index], bytes);
            let clean = redact[index].push(bytes);
            if !clean.is_empty() {
                sink(stream, &clean);
            }
        };

        let result = match &mechanism {
            Mechanism::Direct => held.session.run(channel, &request.command, stdin, timeout, &mut forward).await,
            Mechanism::Sudo { password } => {
                let needs_password = match password {
                    Some(_) => !self.sudo_is_passwordless(&held).await?,
                    None => false,
                };
                let command = escalate::sudo_command(&request.command, needs_password);
                let stdin = match (needs_password, password) {
                    (true, Some(password)) => {
                        let mut bytes = password.expose_secret().as_bytes().to_vec();
                        bytes.push(b'\n');
                        bytes.extend(stdin.unwrap_or_default());
                        Some(bytes)
                    }
                    _ => stdin,
                };
                held.session.run(channel, &command, stdin, timeout, &mut forward).await
            }
            Mechanism::Su { user, password } => {
                if stdin.as_ref().is_some_and(|s| !s.is_empty()) {
                    return Err(Error::Usage("standard input cannot be combined with --su; use --sudo, or put the file and run it".into()));
                }
                let command = escalate::su_command(user, &request.command);
                held.session.run_with_password_prompt(channel, &command, password, self.prompt_timeout, timeout, &mut forward).await
            }
        };
        for (index, stream) in [Stream::Stdout, Stream::Stderr].into_iter().enumerate() {
            let rest = redact[index].finish();
            if !rest.is_empty() {
                sink(stream, &rest);
            }
        }

        let outcome = match result {
            Ok(outcome) => outcome,
            Err(e) => {
                if matches!(e, Error::ConnectionLost { .. }) {
                    self.evict(&ctx.session_key(spec), &held).await;
                }
                return Err(e);
            }
        };
        let tail = |index: usize| String::from_utf8_lossy(&tails[index]).into_owned();
        let failure = match &mechanism {
            Mechanism::Sudo { .. } if outcome.rc != 0 => escalate::sudo_failure(outcome.rc, &tail(1)).map(|r| ("sudo", r)),
            Mechanism::Su { .. } if outcome.rc != 0 => escalate::su_failure(&tail(0)).map(|r| ("su", r)),
            _ => None,
        };
        if let Some((mechanism, reason)) = failure {
            return Err(Error::EscalationFailed { mechanism, reason: reason.to_string() });
        }
        Ok((
            RunReport {
                alias: held.alias.clone(),
                rc: outcome.rc,
                signal: outcome.signal,
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                escalation: mechanism.name().to_string(),
                reconnected,
                warnings: Vec::new(),
            },
            held.login_user.clone(),
        ))
    }

    async fn open_sftp(&self, ctx: &Context, spec: &ServerSpec) -> Result<(Arc<Held>, SftpSession), Error> {
        let (held, channel, _) = self.open_channel(ctx, spec).await?;
        let sftp = Session::sftp_on(channel).await?;
        Ok((held, sftp))
    }

    /// Runs one SFTP operation under the server's policy and records it in the audit log. A
    /// read-only server refuses writes before anything connects.
    async fn file_operation<T>(
        &self,
        ctx: &Context,
        spec: &ServerSpec,
        op: &'static str,
        subject: &str,
        writes: bool,
        body: impl AsyncFnOnce(&SftpSession, &Held) -> Result<T, Error>,
    ) -> Result<(T, Option<String>), Error> {
        let started = Instant::now();
        let mut audit = open_audit(ctx)?;
        let mut user = spec.user.clone().unwrap_or_else(|| "root".into());
        let outcome = async {
            if writes {
                spec.policy.check_write_operation(op)?;
            }
            let (held, sftp) = self.open_sftp(ctx, spec).await?;
            user.clone_from(&held.login_user);
            if writes {
                held.policy.check_write_operation(op)?;
            }
            let result = body(&sftp, &held).await;
            if result.is_err() && held.session.is_closed() {
                self.evict(&ctx.session_key(spec), &held).await;
            }
            result
        }
        .await;
        let logged = audit.write(&Entry {
            alias: &spec.alias,
            user: &user,
            op,
            mechanism: "none",
            rc: outcome.as_ref().ok().map(|_| 0),
            error: outcome.as_ref().err().map(|e| e.code().to_owned()),
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            subject,
            store_subject: ctx.audit_commands,
        });
        Ok((outcome?, logged.err()))
    }

    pub async fn put(&self, ctx: &Context, spec: &ServerSpec, local: &Path, remote: &str, opts: Options) -> Result<TransferReport, Error> {
        let (mut report, warning) = self
            .file_operation(ctx, spec, "put", remote, true, async |sftp, held| transfer::put(sftp, &held.policy, local, remote, opts).await)
            .await?;
        report.warnings.extend(warning);
        Ok(report)
    }

    pub async fn get(&self, ctx: &Context, spec: &ServerSpec, remote: &str, local: &Path, opts: Options) -> Result<TransferReport, Error> {
        let (mut report, warning) = self
            .file_operation(ctx, spec, "get", remote, false, async |sftp, held| transfer::get(sftp, &held.policy, remote, local, opts).await)
            .await?;
        report.warnings.extend(warning);
        Ok(report)
    }

    pub async fn ls(&self, ctx: &Context, spec: &ServerSpec, remote: &str) -> Result<Listing, Error> {
        let (mut listing, warning) = self
            .file_operation(ctx, spec, "ls", remote, false, async |sftp, held| transfer::ls(sftp, &held.policy, remote).await)
            .await?;
        listing.warnings.extend(warning);
        Ok(listing)
    }

    pub async fn cat(&self, ctx: &Context, spec: &ServerSpec, remote: &str, sink: Sink<'_>) -> Result<TransferReport, Error> {
        let (mut report, warning) = self
            .file_operation(ctx, spec, "cat", remote, false, async |sftp, held| {
                let values = held.redaction_values();
                let mut redact = Redactor::new(values.iter().map(String::as_str));
                let report = transfer::cat(sftp, &held.policy, remote, &mut |bytes: &[u8]| {
                    let clean = redact.push(bytes);
                    if !clean.is_empty() {
                        sink(Stream::Stdout, &clean);
                    }
                })
                .await?;
                let rest = redact.finish();
                if !rest.is_empty() {
                    sink(Stream::Stdout, &rest);
                }
                Ok(report)
            })
            .await?;
        report.warnings.extend(warning);
        Ok(report)
    }

    async fn sudo_is_passwordless(&self, held: &Arc<Held>) -> Result<bool, Error> {
        held.passwordless
            .get_or_try_init(|| async {
                let outcome = held.session.exec(escalate::SUDO_PROBE, None, PROBE_TIMEOUT, &mut |_, _| {}).await?;
                Ok::<bool, Error>(outcome.rc == 0)
            })
            .await
            .copied()
    }
}
