use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use futures::StreamExt;
use serde_json::{Map, Value, json};
use sshhh::config::{Config, Server};
use sshhh::daemon::Client;
use sshhh::engine::{Engine, RunRequest, Sink};
use sshhh::error::Error;
use sshhh::hostkeys;
use sshhh::i18n::Catalog;
use sshhh::ops::{self, Op};
use sshhh::session::{Stream, command_from_args, shell_quote};
use sshhh::spec::{Context, ServerSpec};
use sshhh::transfer::Options;

use super::{Global, Setup, load};

const FAN_OUT: usize = 8;

pub enum Backend {
    Local(Engine),
    Daemon(Client),
}

impl Backend {
    async fn exec(&self, ctx: &Context, spec: &ServerSpec, op: &Op, stdin: Option<&[u8]>, sink: Sink<'_>) -> Result<Value, Error> {
        match self {
            Backend::Local(engine) => ops::execute(engine, ctx, spec, op, stdin.map(<[u8]>::to_vec), sink).await,
            Backend::Daemon(client) => client.exec(ctx, spec, op, stdin, sink).await,
        }
    }

    async fn finish(&self) {
        if let Backend::Local(engine) = self {
            engine.shutdown().await;
        }
    }
}

pub struct Prepared {
    pub setup: Setup,
    pub specs: Vec<ServerSpec>,
    pub ctx: Context,
    pub backend: Backend,
}

pub fn select<'a>(config: &'a Config, global: &Global, alias: Option<&str>) -> Result<Vec<&'a Server>, Error> {
    if global.all {
        if config.servers.is_empty() {
            return Err(Error::NoConfig { searched: String::new() });
        }
        return Ok(config.servers.values().collect());
    }
    if !global.on.is_empty() {
        let mut picked: Vec<&Server> = Vec::new();
        for name in &global.on {
            let server = config.select(Some(name))?;
            if !picked.iter().any(|s| s.alias == server.alias) {
                picked.push(server);
            }
        }
        return Ok(picked);
    }
    Ok(vec![config.select(alias.or(global.server.as_deref()))?])
}

pub fn context(setup: &Setup, specs: &[ServerSpec], global: &Global) -> Context {
    let mut env = BTreeMap::new();
    let names = specs.iter().flat_map(ServerSpec::env_references).chain(["BW_SESSION".to_string()]);
    for name in names {
        if let Ok(value) = std::env::var(&name) {
            env.insert(name, value);
        }
    }
    Context {
        settings: setup.config.settings.clone(),
        env,
        home: Some(setup.home.clone()),
        cwd: setup.cwd.clone(),
        known_hosts: hostkeys::default_path(&setup.home),
        accept_new: global.accept_new,
        connect_timeout_secs: global.connect_timeout,
        audit_commands: global.audit_commands,
    }
}

pub fn client(setup: &Setup) -> Result<Client, Error> {
    let exe = std::env::current_exe().map_err(|e| Error::Io(format!("cannot locate the sshhh executable: {e}")))?;
    Ok(Client::new(&setup.home, exe))
}

fn prepare(global: &Global) -> Result<Prepared, Error> {
    let setup = load(global)?;
    let specs: Vec<ServerSpec> = select(&setup.config, global, None)?.into_iter().map(ServerSpec::from_server).collect();
    let ctx = context(&setup, &specs, global);
    let backend = if global.no_daemon { Backend::Local(Engine::new()) } else { Backend::Daemon(client(&setup)?) };
    Ok(Prepared { setup, specs, ctx, backend })
}

#[derive(Default)]
struct Capture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct Done {
    alias: String,
    outcome: Result<Value, Error>,
    capture: Capture,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Run,
    Cat,
    Put,
    Get,
    Ls,
}

impl Kind {
    fn streams(self) -> bool {
        matches!(self, Kind::Run | Kind::Cat)
    }
}

async fn drive(p: &Prepared, global: &Global, kind: Kind, op: Op, stdin: Option<Vec<u8>>) -> Vec<Done> {
    let live = p.specs.len() == 1 && !global.json && kind.streams();
    let results = futures::stream::iter(p.specs.iter())
        .map(|spec| {
            let (op, stdin) = (&op, stdin.as_deref());
            async move {
                let mut capture = Capture::default();
                let outcome = if live {
                    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
                    let mut sink = |stream: Stream, bytes: &[u8]| {
                        let _ = match stream {
                            Stream::Stdout => out.write_all(bytes).and_then(|()| out.flush()),
                            Stream::Stderr => err.write_all(bytes).and_then(|()| err.flush()),
                        };
                    };
                    p.backend.exec(&p.ctx, spec, op, stdin, &mut sink).await
                } else {
                    let mut sink = |stream: Stream, bytes: &[u8]| match stream {
                        Stream::Stdout => capture.stdout.extend_from_slice(bytes),
                        Stream::Stderr => capture.stderr.extend_from_slice(bytes),
                    };
                    p.backend.exec(&p.ctx, spec, op, stdin, &mut sink).await
                };
                Done { alias: spec.alias.clone(), outcome, capture }
            }
        })
        .buffered(FAN_OUT)
        .collect::<Vec<_>>()
        .await;
    p.backend.finish().await;
    results
}

fn absolute(path: PathBuf, cwd: &Path) -> PathBuf {
    if path.is_absolute() { path } else { cwd.join(path) }
}

fn put_text(object: &mut Map<String, Value>, name: &str, bytes: &[u8]) {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            object.insert("lossy_output".to_string(), Value::Bool(true));
            String::from_utf8_lossy(bytes).into_owned()
        }
    };
    object.insert(name.to_string(), Value::String(text));
}

fn envelope(done: &Done, kind: Kind) -> Value {
    let value = match &done.outcome {
        Err(e) => return json!({ "alias": done.alias, "error": e.code(), "message": e.to_string() }),
        Ok(value) => value,
    };
    let mut object = match value {
        Value::Object(object) => object.clone(),
        other => Map::from_iter([("result".to_string(), other.clone())]),
    };
    object.insert("alias".to_string(), Value::String(done.alias.clone()));
    if kind.streams() {
        put_text(&mut object, "stdout", &done.capture.stdout);
    }
    if kind == Kind::Run {
        put_text(&mut object, "stderr", &done.capture.stderr);
    }
    Value::Object(object)
}

fn rc_of(value: &Value) -> i32 {
    value.get("rc").and_then(Value::as_i64).map_or(0, |rc| i32::try_from(rc).unwrap_or(-1))
}

fn exit_code(rc: i32) -> u8 {
    u8::try_from(rc).unwrap_or(255)
}

fn plain(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_string)
}

fn show(done: &Done, kind: Kind, tr: &Catalog) {
    let Ok(value) = &done.outcome else { return };
    let _ = std::io::stdout().write_all(&done.capture.stdout);
    let _ = std::io::stderr().write_all(&done.capture.stderr);
    let field = |name: &str| plain(value.get(name).unwrap_or(&Value::Null));
    match kind {
        Kind::Put => println!("{}", tr.t("put.done", &[("alias", &done.alias), ("bytes", &field("bytes")), ("remote", &field("remote"))])),
        Kind::Get => println!("{}", tr.t("get.done", &[("alias", &done.alias), ("bytes", &field("bytes")), ("local", &field("local"))])),
        Kind::Ls => {
            for entry in value.get("entries").and_then(Value::as_array).into_iter().flatten() {
                let text = |name: &str| entry.get(name).filter(|v| !v.is_null()).map_or_else(|| "-".to_string(), plain);
                println!("{}", tr.t("ls.entry", &[("kind", &text("kind")), ("mode", &text("mode")), ("size", &text("size")), ("name", &text("name"))]));
            }
            let hidden = value.get("hidden_by_policy").and_then(Value::as_u64).unwrap_or(0);
            if hidden > 0 {
                eprintln!("{}", tr.t("ls.hidden", &[("count", &hidden)]));
            }
        }
        Kind::Run | Kind::Cat => {}
    }
}

/// Prints the results of one or many servers and picks the exit code: a single run passes the
/// remote status through, a failure of the tool itself is returned as the error.
fn finish(mut done: Vec<Done>, global: &Global, kind: Kind, tr: &Catalog) -> Result<u8, Error> {
    if done.len() == 1 {
        let Done { alias, outcome, capture } = done.remove(0);
        let only = Done { alias, outcome: Ok(outcome?), capture };
        if global.json {
            println!("{}", envelope(&only, kind));
        } else {
            show(&only, kind, tr);
        }
        let status = only.outcome.as_ref().ok().filter(|_| kind == Kind::Run).map_or(0, |v| exit_code(rc_of(v)));
        return Ok(status);
    }

    let mut status = 0u8;
    let mut tool_error = false;
    for entry in &done {
        match &entry.outcome {
            Err(_) => tool_error = true,
            Ok(value) if kind == Kind::Run && status == 0 => status = exit_code(rc_of(value)),
            Ok(_) => {}
        }
    }
    if global.json {
        println!("{}", Value::Array(done.iter().map(|d| envelope(d, kind)).collect()));
    } else {
        for entry in &done {
            match &entry.outcome {
                Err(e) => {
                    println!("{}", tr.t("run.header_failed", &[("alias", &entry.alias)]));
                    super::print_error(e);
                }
                Ok(value) => {
                    println!("{}", tr.t("run.header", &[("alias", &entry.alias), ("rc", &rc_of(value))]));
                    show(entry, kind, tr);
                }
            }
        }
    }
    Ok(if tool_error { 255 } else { status })
}

pub async fn command(global: &Global, words: Vec<String>, tr: &Catalog) -> Result<u8, Error> {
    let p = prepare(global)?;
    let stdin = if global.stdin { Some(read_stdin().await?) } else { None };
    let request = RunRequest { command: command_from_args(&words), root: global.root_mode(), timeout_secs: global.timeout };
    finish(drive(&p, global, Kind::Run, Op::Run(request), stdin).await, global, Kind::Run, tr)
}

pub async fn script(global: &Global, file: &str, args: &[String], tr: &Catalog) -> Result<u8, Error> {
    let p = prepare(global)?;
    let body = if file == "-" {
        read_stdin().await?
    } else {
        let path = absolute(PathBuf::from(file), &p.setup.cwd);
        tokio::fs::read(&path).await.map_err(|e| Error::Io(format!("cannot read {}: {}", path.display(), e.kind())))?
    };
    let mut command = "sh -s".to_string();
    if !args.is_empty() {
        command.push_str(" --");
        for arg in args {
            command.push(' ');
            command.push_str(&shell_quote(arg));
        }
    }
    let request = RunRequest { command, root: global.root_mode(), timeout_secs: global.timeout };
    finish(drive(&p, global, Kind::Run, Op::Run(request), Some(body)).await, global, Kind::Run, tr)
}

pub async fn put(global: &Global, local: PathBuf, remote: String, opts: Options, tr: &Catalog) -> Result<u8, Error> {
    let p = prepare(global)?;
    let op = Op::Put { local: absolute(local, &p.setup.cwd), remote, opts };
    finish(drive(&p, global, Kind::Put, op, None).await, global, Kind::Put, tr)
}

pub async fn get(global: &Global, remote: String, local: PathBuf, opts: Options, tr: &Catalog) -> Result<u8, Error> {
    let p = prepare(global)?;
    if p.specs.len() != 1 {
        return Err(Error::Usage("get downloads from one server; select it with -s".into()));
    }
    let op = Op::Get { remote, local: absolute(local, &p.setup.cwd), opts };
    finish(drive(&p, global, Kind::Get, op, None).await, global, Kind::Get, tr)
}

pub async fn ls(global: &Global, remote: String, tr: &Catalog) -> Result<u8, Error> {
    let p = prepare(global)?;
    finish(drive(&p, global, Kind::Ls, Op::Ls { remote }, None).await, global, Kind::Ls, tr)
}

pub async fn cat(global: &Global, remote: String, tr: &Catalog) -> Result<u8, Error> {
    let p = prepare(global)?;
    let done = drive(&p, global, Kind::Cat, Op::Cat { remote }, None).await;
    if global.json && done.iter().any(|d| d.outcome.is_ok() && std::str::from_utf8(&d.capture.stdout).is_err()) {
        return Err(Error::Usage("the file is not valid UTF-8, so --json cannot carry it; use get".into()));
    }
    finish(done, global, Kind::Cat, tr)
}

async fn read_stdin() -> Result<Vec<u8>, Error> {
    use tokio::io::AsyncReadExt;
    let mut data = Vec::new();
    tokio::io::stdin().read_to_end(&mut data).await.map_err(|e| Error::Io(format!("cannot read standard input: {e}")))?;
    Ok(data)
}
