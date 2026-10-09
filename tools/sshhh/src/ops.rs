use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::{Engine, RunRequest, Sink};
use crate::error::Error;
use crate::spec::{Context, ServerSpec};
use crate::transfer::Options;

/// One unit of work against one server; the same value travels to the daemon or runs in-process.
#[derive(Clone, Serialize, Deserialize)]
pub enum Op {
    Run(RunRequest),
    Put { local: PathBuf, remote: String, opts: Options },
    Get { remote: String, local: PathBuf, opts: Options },
    Ls { remote: String },
    Cat { remote: String },
}

pub async fn execute(engine: &Engine, ctx: &Context, spec: &ServerSpec, op: &Op, stdin: Option<Vec<u8>>, sink: Sink<'_>) -> Result<Value, Error> {
    let json = |e: serde_json::Error| Error::Io(format!("cannot encode the result: {e}"));
    match op {
        Op::Run(request) => serde_json::to_value(engine.run(ctx, spec, request, stdin, sink).await?).map_err(json),
        Op::Put { local, remote, opts } => serde_json::to_value(engine.put(ctx, spec, local, remote, *opts).await?).map_err(json),
        Op::Get { remote, local, opts } => serde_json::to_value(engine.get(ctx, spec, remote, local, *opts).await?).map_err(json),
        Op::Ls { remote } => serde_json::to_value(engine.ls(ctx, spec, remote).await?).map_err(json),
        Op::Cat { remote } => serde_json::to_value(engine.cat(ctx, spec, remote, sink).await?).map_err(json),
    }
}
