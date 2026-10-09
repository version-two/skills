# sshhh – plan

Append-only. Only `[ ]` becomes `[x]` after verified completion; notes go in `<!-- NOTE: ... -->`.

## v0.1 build

- [x] Cargo crate `sshhh-cli` (bin `sshhh`) in the `tools` workspace, release-safe dependency tree (no aws-lc, openssl, dbus) <!-- NOTE: verified with cargo tree for x86_64/aarch64 musl and aarch64-apple-darwin -->
- [ ] clap skeleton, `--version`, JSON error contract on stderr, exit codes
- [ ] `.env` parser: alias table, lookup order (`--env`, `SSHHH_ENV`, `./.local/.env`, `./.env` walking up, `~/.config/sshhh/servers.env`), process environment wins
- [x] Legacy keys accepted with a deprecation warning
- [x] Multi-server `.env` (`ALIAS_KEY` prefixes, `DEFAULT`)
- [ ] `run`: russh connect, real remote exit status, separate stdout/stderr, `--json` envelope, timeout
- [ ] Auth: password, keyboard-interactive, OpenSSH key, PuTTY `.ppk`, certificate, agent
- [ ] Host keys: strict, own `known_hosts`, `trust`, `forget`, changed key is a hard failure
- [ ] Secret provider trait; reference syntax `scheme://...` in any credential value <!-- NOTE: reference syntax works (secrets.rs, tested); dispatch is an enum (Target) instead of a trait because there are three in-process providers – needs the user's call before ticking -->
- [ ] Provider `bw://` (Bitwarden CLI, cloud and self-hosted), `unlock`, `bw_wrong_server`, own `BITWARDENCLI_APPDATA_DIR`
- [x] Provider `env://`
- [x] Provider `file://`
- [ ] `put` / `get` via SFTP, `--verify` (sha256), `--private`, streaming
- [ ] `--root`: `sudo -S` (password over stdin, never in the command string)
- [ ] `--root`: `su -l` on a PTY with prompt detection; `ESCALATE=su|sudo|none`
- [ ] Daemon: per-user, one session per alias+config hash, TTL, keepalive, idle exit; `connection_lost` never silently re-runs
- [ ] `--no-daemon` mode
- [ ] `doctor`, `import` (legacy `.env` migration), output redaction, audit log
- [x] In-process russh server integration tests (CI has no sshd) <!-- NOTE: exec, escalation and a filesystem-backed SFTP server; suite verified on Windows and on Linux (WSL) -->
<!-- NOTE: a real sshd run (WSL Ubuntu) for sudo and su is still open, tracked below -->

- [ ] Plugin: `plugins/sshhh` (plugin.json, launcher, SKILL.md), marketplace entry, release workflow tag glob, README row
- [ ] Tag `sshhh-v0.1.0`, release workflow green, launcher downloads and verifies the binary
- [x] `READONLY=true` per server (user request): enforced inside the engine, no flag or later config layer can lower it; refuses `put`, `--root`, output redirection and any command not listed in `ALLOW_COMMANDS` <!-- NOTE: engine and config verified by tests; CLI flags are added in main.rs and none of them touches the policy. A catch-all ALLOW_COMMANDS entry is rejected under READONLY. -->

- [x] Per-server access policy in the `.env` (user request): `ALLOW_COMMANDS`, `DENY_COMMANDS`, `ALLOW_PATHS`, `DENY_PATHS` (`;`-separated, globs; `none` disables); deny wins, every config layer can only tighten; enforced for `run`, `put`, `get`, `ls`, `cat` on the realpath of the server (symlinks resolved)
- [ ] Read-only SFTP commands `ls` and `cat` so a read-only server stays inspectable <!-- NOTE: added by the READONLY request; not in the original surface -->
- [ ] Policy limits documented honestly: command lists are checked on a parsed argv (no sandbox against a hostile account), bare relative words cannot be known to be files, whoever can supply a different `--env` can supply a different policy

## Blocked on the user

- [ ] Real `bw login` / `bw unlock` / `bw get` round trip against a self-hosted Bitwarden server (needs an account; the CLI side is verified without one)
- [ ] Open bw questions needing a logged-in account: `fields[]` shape of `bw get item`, behaviour on multiple name matches, `locked` / `unlocked` status strings, Vaultwarden behaviour, spawn latency

## Deferred secret providers (not built in v0.1)

- [ ] `bws://` Bitwarden Secrets Manager
- [ ] `op://` 1Password CLI
- [ ] `vault://` HashiCorp Vault
- [ ] `az://` Azure Key Vault
- [ ] `aws-sm://` AWS Secrets Manager
- [ ] `cred://` Windows Credential Manager / OS keyring
- [ ] `gcloud://` Google Secret Manager
- [ ] `pass://` / `gopass://`
- [ ] `keepassxc://`
- [ ] `doppler://`
- [ ] `infisical://`
- [ ] `sops://` / `age://`

## v1.5 (not built in v0.1)

- [ ] `fwd -L` port forwarding (direct-tcpip)
- [ ] `shell` interactive PTY
- [ ] `JUMP=` bastion hosts
- [ ] Directory transfer extras (recursive, resume)
- [ ] `--follow` streaming output
