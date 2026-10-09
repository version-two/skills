mod admin;
mod run;

use std::path::PathBuf;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use serde_json::json;
use sshhh::config::{Config, LoadOptions, process_env_from_os};
use sshhh::error::Error;
use sshhh::escalate::RootMode;
use sshhh::i18n::{Catalog, LANG_VAR};
use sshhh::transfer::Options;

#[derive(Parser)]
#[command(
    name = "sshhh",
    version,
    about = "Quiet, fast SSH for agents and scripts",
    long_about = "Runs commands and moves files on servers described by a .env, over a persistent connection held by a per-user daemon. Output is JSON with --json; failures of the tool itself exit 255.",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub command: Option<Command>,
    /// The remote command. A single word is passed to the remote shell as written; several words are quoted one by one. Put `--` before a command that starts with a subcommand name
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "COMMAND")]
    pub run: Vec<String>,
}

#[derive(Args, Clone)]
pub struct Global {
    /// Server alias (default: DEFAULT in the .env, SSH_SERVER, or the only server)
    #[arg(short = 's', long = "server", global = true, value_name = "ALIAS")]
    pub server: Option<String>,
    /// Run on every configured server
    #[arg(long, global = true, conflicts_with_all = ["server", "on"])]
    pub all: bool,
    /// Run on these aliases, comma separated
    #[arg(long, global = true, value_delimiter = ',', value_name = "A,B")]
    pub on: Vec<String>,
    /// Read only this file instead of searching for .env files
    #[arg(long, global = true, env = "SSHHH_ENV", value_name = "FILE")]
    pub env: Option<PathBuf>,
    /// Machine-readable output
    #[arg(long, global = true)]
    pub json: bool,
    /// Command timeout in seconds
    #[arg(long, global = true, default_value_t = 120, value_name = "SECONDS")]
    pub timeout: u64,
    /// Connect timeout in seconds
    #[arg(long, global = true, default_value_t = 15, value_name = "SECONDS")]
    pub connect_timeout: u64,
    /// Run as root, using sudo or su as the configured secrets allow
    #[arg(long, global = true, conflicts_with_all = ["sudo", "su"])]
    pub root: bool,
    /// Run as root through sudo
    #[arg(long, global = true, conflicts_with = "su")]
    pub sudo: bool,
    /// Run as root through su
    #[arg(long, global = true)]
    pub su: bool,
    /// Trust the host key of a server seen for the first time (a changed key is still refused)
    #[arg(long, global = true)]
    pub accept_new: bool,
    /// Record command text in the audit log, not only its hash
    #[arg(long, global = true)]
    pub audit_commands: bool,
    /// Use a private connection for this call instead of the daemon
    #[arg(long, global = true)]
    pub no_daemon: bool,
    /// Send this process's standard input to the remote command
    #[arg(long, global = true)]
    pub stdin: bool,
}

impl Global {
    pub fn root_mode(&self) -> Option<RootMode> {
        match (self.root, self.sudo, self.su) {
            (_, true, _) => Some(RootMode::Sudo),
            (_, _, true) => Some(RootMode::Su),
            (true, _, _) => Some(RootMode::Auto),
            _ => None,
        }
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Run a local script on the server through `sh -s`, without a temporary file
    Script {
        /// Script file, or - for standard input
        file: String,
        /// Arguments for the script
        args: Vec<String>,
    },
    /// Upload a file
    Put {
        local: PathBuf,
        remote: String,
        /// Hash the remote copy before it replaces the target
        #[arg(long)]
        verify: bool,
        /// Give the remote file mode 0600
        #[arg(long)]
        private: bool,
    },
    /// Download a file
    Get {
        remote: String,
        local: PathBuf,
        /// Hash the remote file again after the download
        #[arg(long)]
        verify: bool,
        /// Make the local file readable by you only
        #[arg(long)]
        private: bool,
    },
    /// List a remote directory
    Ls { remote: String },
    /// Print a remote file
    Cat { remote: String },
    /// Show the configured servers without any secret value
    List,
    /// Check configuration, permissions, host keys and vault access
    Doctor { alias: Option<String> },
    /// Learn the host key of a server
    Trust {
        alias: Option<String>,
        /// Accept without asking when the server presents exactly this fingerprint
        #[arg(long, value_name = "SHA256:...")]
        fingerprint: Option<String>,
    },
    /// Forget the stored host key of a server
    Forget { alias: Option<String> },
    /// Manage the background connection holder
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Unlock a vault and keep its session in the daemon
    Unlock {
        #[command(subcommand)]
        vault: Vault,
    },
    /// Rewrite legacy key names in a .env file
    Import {
        /// A .env file, or a directory holding one
        path: PathBuf,
        /// Show the changes without writing
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum DaemonAction {
    Status,
    Stop,
    #[command(hide = true)]
    Run {
        #[arg(long, default_value_t = 600)]
        idle: u64,
    },
}

#[derive(Subcommand)]
pub enum Vault {
    /// Bitwarden CLI
    Bw,
}

pub fn print_error(error: &Error) {
    eprintln!("{}", json!({ "error": error.code(), "message": error.to_string() }));
}

pub fn fail(error: &Error) -> u8 {
    print_error(error);
    error.exit_code()
}

pub struct Setup {
    pub config: Config,
    pub home: PathBuf,
    pub cwd: PathBuf,
}

pub fn home() -> Result<PathBuf, Error> {
    std::env::home_dir().ok_or_else(|| Error::Usage("cannot determine the home directory".into()))
}

pub fn load(global: &Global) -> Result<Setup, Error> {
    let home = home()?;
    let cwd = std::env::current_dir().map_err(|e| Error::Io(format!("cannot read the working directory: {e}")))?;
    let config = Config::load(&LoadOptions {
        env_file: global.env.clone(),
        cwd: cwd.clone(),
        home: Some(home.clone()),
        process_env: process_env_from_os(),
    })?;
    for warning in &config.warnings {
        eprintln!("{}", serde_json::to_string(warning).unwrap_or_default());
    }
    Ok(Setup { config, home, cwd })
}

pub async fn main() -> u8 {
    let matches = match Cli::command().try_get_matches() {
        Ok(matches) => matches,
        Err(e) if !e.use_stderr() => {
            let _ = e.print();
            return 0;
        }
        Err(e) => return fail(&Error::Usage(e.render().to_string().trim().to_string())),
    };
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(e) => return fail(&Error::Usage(e.to_string())),
    };
    let language = std::env::var(LANG_VAR).ok();
    let catalog = match Catalog::load(language.as_deref()) {
        Ok(catalog) => catalog,
        Err(e) => return fail(&e),
    };
    let outcome = match cli.command {
        None if cli.run.is_empty() => Err(Error::Usage("no command given; see --help".into())),
        None => run::command(&cli.global, cli.run, &catalog).await,
        Some(Command::Script { file, args }) => run::script(&cli.global, &file, &args, &catalog).await,
        Some(Command::Put { local, remote, verify, private }) => run::put(&cli.global, local, remote, Options { verify, private }, &catalog).await,
        Some(Command::Get { remote, local, verify, private }) => run::get(&cli.global, remote, local, Options { verify, private }, &catalog).await,
        Some(Command::Ls { remote }) => run::ls(&cli.global, remote, &catalog).await,
        Some(Command::Cat { remote }) => run::cat(&cli.global, remote, &catalog).await,
        Some(Command::List) => admin::list(&cli.global, &catalog),
        Some(Command::Doctor { alias }) => admin::doctor(&cli.global, alias, &catalog).await,
        Some(Command::Trust { alias, fingerprint }) => admin::trust(&cli.global, alias, fingerprint, &catalog).await,
        Some(Command::Forget { alias }) => admin::forget(&cli.global, alias, &catalog).await,
        Some(Command::Daemon { action }) => admin::daemon(action, &cli.global, &catalog).await,
        Some(Command::Unlock { vault: Vault::Bw }) => admin::unlock_bw(&cli.global, &catalog).await,
        Some(Command::Import { path, dry_run }) => admin::import(&cli.global, &path, dry_run, &catalog),
    };
    match outcome {
        Ok(code) => code,
        Err(e) => fail(&e),
    }
}
