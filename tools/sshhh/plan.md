# sshhh – plan

Append-only. Only `[ ]` becomes `[x]` after verified completion; notes go in `<!-- NOTE: ... -->`.

## v0.1 build

- [ ] Cargo crate `sshhh-cli` (bin `sshhh`) in the `tools` workspace, release-safe dependency tree (no aws-lc, openssl, dbus)
- [ ] clap skeleton, `--version`, JSON error contract on stderr, exit codes
- [ ] `.env` parser: alias table, lookup order (`--env`, `SSHHH_ENV`, `./.local/.env`, `./.env` walking up, `~/.config/sshhh/servers.env`), process environment wins
- [ ] Legacy keys accepted with a deprecation warning
- [ ] Multi-server `.env` (`ALIAS_KEY` prefixes, `DEFAULT`)
- [ ] `run`: russh connect, real remote exit status, separate stdout/stderr, `--json` envelope, timeout
- [ ] Auth: password, keyboard-interactive, OpenSSH key, PuTTY `.ppk`, certificate, agent
- [ ] Host keys: strict, own `known_hosts`, `trust`, `forget`, changed key is a hard failure
- [ ] Secret provider trait; reference syntax `scheme://...` in any credential value
- [ ] Provider `bw://` (Bitwarden CLI, cloud and self-hosted), `unlock`, `bw_wrong_server`, own `BITWARDENCLI_APPDATA_DIR`
- [ ] Provider `env://`
- [ ] Provider `file://`
- [ ] `put` / `get` via SFTP, `--verify` (sha256), `--private`, streaming
- [ ] `--root`: `sudo -S` (password over stdin, never in the command string)
- [ ] `--root`: `su -l` on a PTY with prompt detection; `ESCALATE=su|sudo|none`
- [ ] Daemon: per-user, one session per alias+config hash, TTL, keepalive, idle exit; `connection_lost` never silently re-runs
- [ ] `--no-daemon` mode
- [ ] `doctor`, `import` (legacy `.env` migration), output redaction, audit log
- [ ] In-process russh server integration tests (CI has no sshd)
- [ ] Plugin: `plugins/sshhh` (plugin.json, launcher, SKILL.md), marketplace entry, release workflow tag glob, README row
- [ ] Tag `sshhh-v0.1.0`, release workflow green, launcher downloads and verifies the binary

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
