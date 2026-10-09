use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use interprocess::local_socket::tokio::{Listener, Stream as Socket};
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
use interprocess::local_socket::{ListenerOptions, Name};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Notify, Semaphore, mpsc};
use zeroize::Zeroizing;

use crate::engine::{Engine, SessionInfo, Sink};
use crate::error::Error;
use crate::ops::{self, Op};
use crate::private;
use crate::session::Stream;
use crate::spec::{Context, ServerSpec, hex};

pub const DEFAULT_IDLE: Duration = Duration::from_secs(600);
const MAX_FRAME: usize = 64 << 20;
const MAX_STDIN: usize = 256 << 20;
const CHUNK: usize = 64 << 10;
const MAX_CONNECTIONS: usize = 64;
const SPAWN_WAIT: Duration = Duration::from_secs(10);
const TOKEN_WAIT: Duration = Duration::from_secs(3);

const REQUEST: u8 = b'R';
const STDIN: u8 = b'S';
const STDOUT: u8 = b'O';
const STDERR: u8 = b'E';
const DONE: u8 = b'D';
const FAILED: u8 = b'X';

#[derive(Serialize, Deserialize)]
pub enum Request {
    Exec(Box<ExecRequest>),
    Status,
    Stop,
    SetBwSession { session: String },
}

#[derive(Serialize, Deserialize)]
pub struct ExecRequest {
    ctx: Context,
    spec: ServerSpec,
    op: Op,
    has_stdin: bool,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    token: String,
    request: Request,
}

#[derive(Serialize, Deserialize)]
struct WireError {
    code: String,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub pid: u32,
    pub uptime_secs: u64,
    pub idle_exit_secs: u64,
    pub bw_session: bool,
    pub sessions: Vec<SessionInfo>,
}

#[derive(Clone)]
pub struct Paths {
    dir: PathBuf,
    token: PathBuf,
    lock: PathBuf,
    #[cfg_attr(windows, allow(dead_code))]
    socket: PathBuf,
    #[cfg_attr(unix, allow(dead_code))]
    pipe: String,
}

impl Paths {
    pub fn new(home: &Path) -> Paths {
        let dir = home.join(".local").join("state").join("sshhh");
        let digest = Sha256::digest(home.to_string_lossy().to_lowercase().as_bytes());
        Paths {
            token: dir.join("daemon.token"),
            lock: dir.join("daemon.lock"),
            socket: dir.join("daemon.sock"),
            pipe: format!("sshhh-{}", &hex(&digest)[..16]),
            dir,
        }
    }

    pub fn token_path(&self) -> &Path {
        &self.token
    }

    #[cfg(unix)]
    fn name(&self) -> std::io::Result<Name<'static>> {
        use interprocess::local_socket::{GenericFilePath, ToFsName};
        self.socket.clone().to_fs_name::<GenericFilePath>()
    }

    #[cfg(windows)]
    fn name(&self) -> std::io::Result<Name<'static>> {
        use interprocess::local_socket::{GenericNamespaced, ToNsName};
        self.pipe.clone().to_ns_name::<GenericNamespaced>()
    }
}

fn io(context: &str) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |e| Error::Io(format!("{context}: {e}"))
}

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, kind: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut head = [kind, 0, 0, 0, 0];
    head[1..].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    w.write_all(&head).await?;
    w.write_all(payload).await?;
    w.flush().await
}

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Option<(u8, Zeroizing<Vec<u8>>)>> {
    let mut head = [0u8; 5];
    match r.read_exact(&mut head).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut payload = Zeroizing::new(vec![0u8; len]);
    r.read_exact(&mut payload).await?;
    Ok(Some((head[0], payload)))
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

struct State {
    engine: Engine,
    token: String,
    bw_session: std::sync::Mutex<Option<SecretString>>,
    started: Instant,
    last_activity: std::sync::Mutex<Instant>,
    active: AtomicUsize,
    stop: Notify,
    permits: Semaphore,
    idle: Duration,
}

impl State {
    fn touch(&self) {
        *self.last_activity.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    }

    fn quiet_for(&self) -> Duration {
        self.last_activity.lock().unwrap_or_else(std::sync::PoisonError::into_inner).elapsed()
    }

    fn bw_session(&self) -> Option<SecretString> {
        self.bw_session.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().map(|s| SecretString::from(s.expose_secret().to_owned()))
    }

    fn set_bw_session(&self, session: Option<SecretString>) {
        *self.bw_session.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = session;
    }
}

fn new_token() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| Error::Io(format!("no random source for the daemon token: {e}")))?;
    Ok(hex(&bytes))
}

fn listener(paths: &Paths) -> Result<Listener, Error> {
    let options = ListenerOptions::new().name(paths.name().map_err(io("daemon socket name"))?).try_overwrite(true);
    #[cfg(unix)]
    let options = interprocess::os::unix::local_socket::ListenerOptionsExt::mode(options, 0o600);
    #[cfg(windows)]
    let options = {
        use interprocess::os::windows::{local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor};
        let sddl = widestring::U16CString::from_str("D:P(A;;GA;;;OW)").map_err(|e| Error::Io(e.to_string()))?;
        options.security_descriptor(SecurityDescriptor::deserialize(&sddl).map_err(io("daemon pipe ACL"))?)
    };
    options.create_tokio().map_err(io("cannot create the daemon socket"))
}

/// Runs the daemon until it is stopped or has been idle for `idle`. Returns at once when another
/// daemon of the same user already holds the lock.
pub async fn serve(home: &Path, idle: Duration) -> Result<(), Error> {
    let paths = Paths::new(home);
    std::fs::create_dir_all(&paths.dir).map_err(io("cannot create the state directory"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&paths.dir, std::fs::Permissions::from_mode(0o700)).map_err(io("cannot restrict the state directory"))?;
    }
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&paths.lock).map_err(io("cannot open the daemon lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(()),
        Err(std::fs::TryLockError::Error(e)) => return Err(Error::Io(format!("cannot lock the daemon lock: {e}"))),
    }
    let _ = std::fs::remove_file(&paths.token);
    let listener = listener(&paths)?;
    let token = new_token()?;
    private::write_private(&paths.token, token.as_bytes()).map_err(io("cannot write the daemon token"))?;

    let state = Arc::new(State {
        engine: Engine::new(),
        token,
        bw_session: std::sync::Mutex::new(None),
        started: Instant::now(),
        last_activity: std::sync::Mutex::new(Instant::now()),
        active: AtomicUsize::new(0),
        stop: Notify::new(),
        permits: Semaphore::new(MAX_CONNECTIONS),
        idle,
    });
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok(conn) = accepted else { continue };
                let state = state.clone();
                tokio::spawn(async move {
                    let Ok(_permit) = state.permits.acquire().await else { return };
                    state.active.fetch_add(1, Ordering::SeqCst);
                    state.touch();
                    handle(conn, &state).await;
                    state.touch();
                    state.active.fetch_sub(1, Ordering::SeqCst);
                });
            }
            _ = state.stop.notified() => break,
            _ = tick.tick() => {
                state.engine.sweep(idle).await;
                let quiet = state.quiet_for() > idle && state.active.load(Ordering::SeqCst) == 0;
                if quiet {
                    state.set_bw_session(None);
                    if state.engine.session_count().await == 0 {
                        break;
                    }
                }
            }
        }
    }
    state.engine.shutdown().await;
    state.set_bw_session(None);
    let _ = std::fs::remove_file(&paths.token);
    drop(listener);
    drop(lock);
    Ok(())
}

async fn handle(conn: Socket, state: &Arc<State>) {
    let (mut rd, mut wr) = tokio::io::split(conn);
    let reply = |result: Result<Value, Error>| match result {
        Ok(value) => (DONE, serde_json::to_vec(&value).unwrap_or_default()),
        Err(e) => (FAILED, serde_json::to_vec(&WireError { code: e.code().to_owned(), message: e.to_string() }).unwrap_or_default()),
    };
    let request = match read_request(&mut rd, &state.token).await {
        Ok(request) => request,
        Err(e) => {
            let (kind, body) = reply(Err(e));
            let _ = write_frame(&mut wr, kind, &body).await;
            return;
        }
    };
    let result = match request {
        Request::Status => status(state).await,
        Request::Stop => {
            state.stop.notify_one();
            Ok(Value::Null)
        }
        Request::SetBwSession { session } => {
            state.set_bw_session(Some(SecretString::from(session)));
            Ok(Value::Null)
        }
        Request::Exec(request) => {
            exec(state, *request, &mut rd, wr).await;
            return;
        }
    };
    let (kind, body) = reply(result);
    let _ = write_frame(&mut wr, kind, &body).await;
}

async fn read_request<R: AsyncRead + Unpin>(rd: &mut R, token: &str) -> Result<Request, Error> {
    let frame = read_frame(rd).await.map_err(io("cannot read the request"))?;
    let Some((REQUEST, body)) = frame else { return Err(Error::Usage("the first frame must be a request".into())) };
    let envelope: Envelope = serde_json::from_slice(&body).map_err(|e| Error::Usage(format!("malformed request: {e}")))?;
    if !same(&envelope.token, token) {
        return Err(Error::Refused("the daemon token was not accepted".into()));
    }
    Ok(envelope.request)
}

async fn status(state: &State) -> Result<Value, Error> {
    let status = Status {
        pid: std::process::id(),
        uptime_secs: state.started.elapsed().as_secs(),
        idle_exit_secs: state.idle.as_secs(),
        bw_session: state.bw_session().is_some(),
        sessions: state.engine.sessions().await,
    };
    serde_json::to_value(status).map_err(|e| Error::Io(e.to_string()))
}

async fn exec<R, W>(state: &Arc<State>, request: ExecRequest, rd: &mut R, mut wr: W)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let ExecRequest { mut ctx, spec, op, has_stdin } = request;
    let stdin = if has_stdin {
        match read_stdin(rd).await {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                let body = serde_json::to_vec(&WireError { code: e.code().to_owned(), message: e.to_string() }).unwrap_or_default();
                let _ = write_frame(&mut wr, FAILED, &body).await;
                return;
            }
        }
    } else {
        None
    };
    if ctx.env.get("BW_SESSION").is_none_or(|v| v.is_empty())
        && let Some(session) = state.bw_session()
    {
        ctx.env.insert("BW_SESSION".into(), session.expose_secret().to_owned());
    }

    let (tx, mut out) = mpsc::channel::<(u8, Vec<u8>)>(16);
    let writer = tokio::spawn(async move {
        while let Some((kind, bytes)) = out.recv().await {
            if write_frame(&mut wr, kind, &bytes).await.is_err() {
                break;
            }
        }
        wr
    });
    let mut forward = |stream: Stream, bytes: &[u8]| {
        let kind = if stream == Stream::Stdout { STDOUT } else { STDERR };
        for chunk in bytes.chunks(CHUNK) {
            if tokio::task::block_in_place(|| tx.blocking_send((kind, chunk.to_vec()))).is_err() {
                return;
            }
        }
    };
    let sink: Sink<'_> = &mut forward;
    let work = ops::execute(&state.engine, &ctx, &spec, &op, stdin, sink);
    let gone = async {
        while let Ok(Some(_)) = read_frame(rd).await {}
    };
    let result = tokio::select! {
        result = work => result,
        _ = gone => Err(Error::Io("the client disconnected".into())),
    };
    drop(tx);
    let Ok(mut wr) = writer.await else { return };
    let (kind, body) = match result {
        Ok(value) => (DONE, serde_json::to_vec(&value).unwrap_or_default()),
        Err(e) => (FAILED, serde_json::to_vec(&WireError { code: e.code().to_owned(), message: e.to_string() }).unwrap_or_default()),
    };
    let _ = write_frame(&mut wr, kind, &body).await;
}

async fn read_stdin<R: AsyncRead + Unpin>(rd: &mut R) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new();
    loop {
        match read_frame(rd).await.map_err(io("cannot read the input"))? {
            Some((STDIN, chunk)) if chunk.is_empty() => return Ok(data),
            Some((STDIN, chunk)) => {
                if data.len() + chunk.len() > MAX_STDIN {
                    return Err(Error::Usage(format!("input is larger than {} MiB", MAX_STDIN >> 20)));
                }
                data.extend_from_slice(&chunk);
            }
            _ => return Err(Error::Usage("the input ended before its terminator".into())),
        }
    }
}

pub struct Client {
    paths: Paths,
    exe: PathBuf,
}

impl Client {
    pub fn new(home: &Path, exe: PathBuf) -> Client {
        Client { paths: Paths::new(home), exe }
    }

    async fn connect(&self) -> std::io::Result<Socket> {
        Socket::connect(self.paths.name()?).await
    }

    async fn read_token(&self) -> Result<String, Error> {
        let deadline = Instant::now() + TOKEN_WAIT;
        loop {
            match std::fs::read_to_string(&self.paths.token) {
                Ok(token) if !token.trim().is_empty() => return Ok(token.trim().to_owned()),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::Io(format!("cannot read the daemon token: {e}"))),
            }
            if Instant::now() > deadline {
                return Err(Error::Io("the daemon did not publish its token".into()));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn spawn(&self) -> Result<(), Error> {
        let mut cmd = std::process::Command::new(&self.exe);
        cmd.args(["daemon", "run"]).env_clear();
        for (key, value) in std::env::vars_os() {
            let upper = key.to_string_lossy().to_ascii_uppercase();
            if SPAWN_ENV.contains(&upper.as_str()) || upper.starts_with("XDG_") {
                cmd.env(key, value);
            }
        }
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0000_0008 | 0x0000_0200);
        cmd.spawn().map(drop).map_err(|e| Error::Io(format!("cannot start the daemon ({}): {e}", self.exe.display())))
    }

    async fn open(&self, start: bool) -> Result<Option<Socket>, Error> {
        if let Ok(conn) = self.connect().await {
            return Ok(Some(conn));
        }
        if !start {
            return Ok(None);
        }
        self.spawn()?;
        let deadline = Instant::now() + SPAWN_WAIT;
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if let Ok(conn) = self.connect().await {
                return Ok(Some(conn));
            }
            if Instant::now() > deadline {
                return Err(Error::Io("the daemon did not come up in time".into()));
            }
        }
    }

    /// `Ok(None)` when no daemon runs and `start` is false.
    pub async fn call(&self, request: Request, stdin: Option<&[u8]>, start: bool, sink: Sink<'_>) -> Result<Option<Value>, Error> {
        let Some(conn) = self.open(start).await? else { return Ok(None) };
        let token = self.read_token().await?;
        let (mut rd, mut wr) = tokio::io::split(conn);
        let body = Zeroizing::new(serde_json::to_vec(&Envelope { token, request }).map_err(|e| Error::Io(e.to_string()))?);
        write_frame(&mut wr, REQUEST, &body).await.map_err(io("cannot send the request"))?;
        if let Some(stdin) = stdin {
            for chunk in stdin.chunks(CHUNK) {
                write_frame(&mut wr, STDIN, chunk).await.map_err(io("cannot send the input"))?;
            }
            write_frame(&mut wr, STDIN, &[]).await.map_err(io("cannot send the input"))?;
        }
        loop {
            let Some((kind, payload)) = read_frame(&mut rd).await.map_err(io("cannot read the reply"))? else {
                return Err(Error::ConnectionLost { host: "the sshhh daemon".into(), reason: "it closed the connection".into() });
            };
            match kind {
                STDOUT => sink(Stream::Stdout, &payload),
                STDERR => sink(Stream::Stderr, &payload),
                DONE => return serde_json::from_slice(&payload).map(Some).map_err(|e| Error::Io(format!("malformed daemon reply: {e}"))),
                FAILED => {
                    let wire: WireError = serde_json::from_slice(&payload).map_err(|e| Error::Io(format!("malformed daemon error: {e}")))?;
                    return Err(Error::Remote { code: wire.code, message: wire.message });
                }
                _ => return Err(Error::Io("unexpected frame from the daemon".into())),
            }
        }
    }

    pub async fn exec(&self, ctx: &Context, spec: &ServerSpec, op: &Op, stdin: Option<&[u8]>, sink: Sink<'_>) -> Result<Value, Error> {
        let request = Request::Exec(Box::new(ExecRequest { ctx: ctx.clone(), spec: spec.clone(), op: op.clone(), has_stdin: stdin.is_some() }));
        self.call(request, stdin, true, sink).await?.ok_or_else(|| Error::Io("the daemon is not running".into()))
    }

    pub async fn status(&self) -> Result<Option<Status>, Error> {
        let mut ignore = |_: Stream, _: &[u8]| {};
        match self.call(Request::Status, None, false, &mut ignore).await? {
            Some(value) => serde_json::from_value(value).map(Some).map_err(|e| Error::Io(format!("malformed daemon status: {e}"))),
            None => Ok(None),
        }
    }

    /// `true` when a daemon was running and has been told to stop.
    pub async fn stop(&self) -> Result<bool, Error> {
        let mut ignore = |_: Stream, _: &[u8]| {};
        Ok(self.call(Request::Stop, None, false, &mut ignore).await?.is_some())
    }

    pub async fn set_bw_session(&self, session: &SecretString) -> Result<(), Error> {
        let mut ignore = |_: Stream, _: &[u8]| {};
        let request = Request::SetBwSession { session: session.expose_secret().to_owned() };
        self.call(request, None, true, &mut ignore).await.map(drop)
    }
}

/// The daemon starts with an empty environment plus these, so nothing the first caller exported
/// (BW_SESSION, tokens) ends up in a long-lived process.
const SPAWN_ENV: [&str; 17] = [
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "TEMP",
    "TMP",
    "TMPDIR",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "USERNAME",
    "USERDOMAIN",
    "LANG",
    "LC_ALL",
];
