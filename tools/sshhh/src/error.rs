use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Usage(String),
    #[error("cannot read {path}: {reason}")]
    EnvRead { path: PathBuf, reason: String },
    #[error("{path}: line {line}: {reason}")]
    EnvParse { path: PathBuf, line: usize, reason: &'static str },
    #[error("no server configured; searched: {searched}")]
    NoConfig { searched: String },
    #[error("unknown server '{alias}'; configured: {known}")]
    UnknownServer { alias: String, known: String },
    #[error("no server selected and none is the default; configured: {known}")]
    NoDefaultServer { known: String },
    #[error("server '{alias}' has no HOST")]
    MissingHost { alias: String },
    #[error("server '{alias}': {key} is set more than once with different values ({names})")]
    ConflictingKeys { alias: String, key: &'static str, names: String },
    #[error("server '{alias}': invalid {key}: {reason}")]
    InvalidValue { alias: String, key: &'static str, reason: String },
    #[error("server '{alias}' has no usable credentials: set KEY, PASS or AUTH=agent")]
    NoCredentials { alias: String },
    #[error("cannot connect to {host}:{port}: {reason}")]
    Connect { host: String, port: u16, reason: String },
    #[error("connecting to {host}:{port} timed out after {secs} s")]
    ConnectTimeout { host: String, port: u16, secs: u64 },
    #[error("host key of {host}:{port} is not trusted yet (fingerprint {fingerprint}); run `sshhh trust`")]
    HostKeyUnknown { host: String, port: u16, fingerprint: String },
    #[error("HOST KEY CHANGED for {host}:{port} (presented {fingerprint}, known_hosts line {line}); refusing to connect, run `sshhh forget` only if the change is expected")]
    HostKeyChanged { host: String, port: u16, fingerprint: String, line: usize },
    #[error("host key of {host}:{port} cannot be checked: {reason}")]
    HostKeyUnusable { host: String, port: u16, reason: String },
    #[error("authentication as '{user}' failed (tried: {tried}); server accepts: {server_methods}")]
    AuthFailed { user: String, tried: String, server_methods: String },
    #[error("cannot use the private key: {reason}")]
    KeyUnusable { reason: String },
    #[error("ssh agent unavailable: {reason}")]
    AgentUnavailable { reason: String },
    #[error("connection to {host} was lost: {reason}")]
    ConnectionLost { host: String, reason: String },
    #[error("the server refused the request: {0}")]
    Refused(String),
    #[error("command timed out after {secs} s; it may still be running on the server")]
    CommandTimeout { secs: u64 },
    #[error("local I/O failed: {0}")]
    Io(String),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Error::Usage(_) => "usage",
            Error::EnvRead { .. } => "env_unreadable",
            Error::EnvParse { .. } => "env_parse",
            Error::NoConfig { .. } => "no_config",
            Error::UnknownServer { .. } => "unknown_server",
            Error::NoDefaultServer { .. } => "no_default_server",
            Error::MissingHost { .. } => "missing_host",
            Error::ConflictingKeys { .. } => "conflicting_keys",
            Error::InvalidValue { .. } => "invalid_value",
            Error::NoCredentials { .. } => "no_credentials",
            Error::Connect { .. } => "connect_failed",
            Error::ConnectTimeout { .. } => "connect_timeout",
            Error::HostKeyUnknown { .. } => "host_key_unknown",
            Error::HostKeyChanged { .. } => "host_key_changed",
            Error::HostKeyUnusable { .. } => "host_key_unusable",
            Error::AuthFailed { .. } => "auth_failed",
            Error::KeyUnusable { .. } => "key_unusable",
            Error::AgentUnavailable { .. } => "agent_unavailable",
            Error::ConnectionLost { .. } => "connection_lost",
            Error::Refused(_) => "refused",
            Error::CommandTimeout { .. } => "command_timeout",
            Error::Io(_) => "io_error",
        }
    }

    pub fn exit_code(&self) -> u8 {
        255
    }
}
