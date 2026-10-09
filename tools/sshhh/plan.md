# sshhh – plan

Append-only. Only `[ ]` becomes `[x]` after verified completion; notes go in `<!-- NOTE: ... -->`.

## v0.1 build

- [x] Cargo crate `sshhh-cli` (bin `sshhh`) in the `tools` workspace, release-safe dependency tree (no aws-lc, openssl, dbus) <!-- NOTE: verified with cargo tree for x86_64/aarch64 musl and aarch64-apple-darwin -->
- [x] clap skeleton, `--version`, JSON error contract on stderr, exit codes <!-- NOTE: tests/cli.rs; clap usage errors become {"error":"usage"} with exit 255 -->
- [x] `.env` parser: alias table, lookup order (`--env`, `SSHHH_ENV`, `./.local/.env`, `./.env` walking up, `~/.config/sshhh/servers.env`), process environment wins <!-- NOTE: deviations: only SSHHH_-prefixed process variables override files (plus SSH_SERVER for the default alias); an explicit --env file is exclusive; BW_BIN/BW_APPDATA/BW_SERVER are honoured only from trusted sources -->

- [x] Legacy keys accepted with a deprecation warning
- [x] Multi-server `.env` (`ALIAS_KEY` prefixes, `DEFAULT`)
- [x] `run`: russh connect, real remote exit status, separate stdout/stderr, `--json` envelope, timeout <!-- NOTE: a single word is passed to the remote shell verbatim and several words are quoted one by one, so the planned --raw flag is not needed and was dropped -->
- [x] Auth: password, keyboard-interactive, OpenSSH key, PuTTY `.ppk`, certificate, agent <!-- NOTE: all verified against a real OpenSSH server in WSL (ed25519, RSA, encrypted, .ppk with and without passphrase, user certificate, agent, kbdint, password, wrong password, missing passphrase). Windows OpenSSH agent pipe and Pageant not exercised. -->
- [x] Host keys: strict, own `known_hosts`, `trust`, `forget`, changed key is a hard failure <!-- NOTE: host certificates are not supported -->

- [ ] Secret provider trait; reference syntax `scheme://...` in any credential value <!-- NOTE: reference syntax works (secrets.rs, tested); dispatch is an enum (Target) instead of a trait because there are three in-process providers – needs the user's call before ticking -->
- [ ] Provider `bw://` (Bitwarden CLI, cloud and self-hosted), `unlock`, `bw_wrong_server`, own `BITWARDENCLI_APPDATA_DIR`
- [x] Provider `env://`
- [x] Provider `file://`
- [x] `put` / `get` via SFTP, `--verify` (sha256), `--private`, streaming <!-- NOTE: regular files only; --root/--sudo/--su/--stdin are refused for put/get/ls/cat. Windows get --private against a real remote not exercised (ACL helper tested locally). -->
- [x] `--root`: `sudo -S` (password over stdin, never in the command string) <!-- NOTE: verified against a real sudo in WSL and the in-process server -->
- [x] `--root`: `su -l` on a PTY with prompt detection; `ESCALATE=su|sudo|none` <!-- NOTE: verified against a real su in WSL; stderr is merged into stdout and stdin is refused under su -->
- [x] Daemon: per-user, one session per alias+config hash, TTL, keepalive, idle exit; `connection_lost` never silently re-runs <!-- NOTE: the session key hashes the unresolved spec; the pipe DACL names the user's SID; cross-access between an elevated and a normal token of the same user was inspected, not exercised -->
- [x] `--no-daemon` mode
- [x] `doctor`, `import` (legacy `.env` migration), output redaction, audit log <!-- NOTE: `import --to <vault>` is deferred (see below); the Windows ACL check in doctor relies on English icacls output -->

- [x] In-process russh server integration tests (CI has no sshd) <!-- NOTE: exec, escalation and a filesystem-backed SFTP server; suite verified on Windows and on Linux (WSL) -->
<!-- NOTE: a real sshd run (WSL Ubuntu) for sudo and su is still open, tracked below -->

- [x] Plugin: `plugins/sshhh` (plugin.json, launcher, SKILL.md), marketplace entry, release workflow tag glob, README row <!-- NOTE: JSON and shell syntax checked; static musl build, tests and clippy --locked pass in WSL; the launcher download is verified only by the release run -->

- [x] Tag `sshhh-v0.1.0`, release workflow green, launcher downloads and verifies the binary <!-- NOTE: first run failed on a Linux-only test race (text file busy on the copied bw stub); fixed in 694ec4e and the tag moved before any release existed. Launcher checked on Windows against the published asset. -->

- [x] `READONLY=true` per server (user request): enforced inside the engine, no flag or later config layer can lower it; refuses `put`, `--root`, output redirection and any command not listed in `ALLOW_COMMANDS` <!-- NOTE: engine and config verified by tests; CLI flags are added in main.rs and none of them touches the policy. A catch-all ALLOW_COMMANDS entry is rejected under READONLY. -->

- [x] Per-server access policy in the `.env` (user request): `ALLOW_COMMANDS`, `DENY_COMMANDS`, `ALLOW_PATHS`, `DENY_PATHS` (`;`-separated, globs; `none` disables); deny wins, every config layer can only tighten; enforced for `run`, `put`, `get`, `ls`, `cat` on the realpath of the server (symlinks resolved)
- [x] Read-only SFTP commands `ls` and `cat` so a read-only server stays inspectable <!-- NOTE: added by the READONLY request; not in the original surface -->
- [x] Policy limits documented honestly: command lists are checked on a parsed argv (no sandbox against a hostile account), bare relative words cannot be known to be files, whoever can supply a different `--env` can supply a different policy <!-- NOTE: in SKILL.md and docs/DESIGN.md. Also: stdin (scripts, --stdin) is refused under any policy (rule STDIN) and a --stdin flag was added. -->
- [x] i18n from day one: `locales/en.json` catalog, `SSHHH_LANG`, test that keys used and keys defined match <!-- NOTE: covers human CLI text only; error messages and JSON codes stay English -->
- [ ] `put`/`get` of root-owned files by streaming `cat`/`tee` over an escalated channel <!-- NOTE: planned in the design but not built; --root is refused for file commands instead of ignored -->
- [ ] `import --to bw` (move literal passwords into the vault) <!-- NOTE: deferred -->
- [ ] Real `sshhh unlock bw` flow exercised against a real `bw` with a logged-in account

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
