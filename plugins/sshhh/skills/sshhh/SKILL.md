---
name: sshhh
description: Run commands, scripts and file transfers on remote servers over SSH with the bundled `sshhh` CLI, which reads server definitions from a `.env`, keeps connections open between calls, escalates with sudo or su and can take credentials from Bitwarden. Use when the user wants to run something on a server, deploy, read logs, copy files to or from a host, check several servers at once, or set a server read-only or restrict what an agent may run there. Triggers: ssh, sftp, scp, sudo on a server, "on the server", .env with HOST / USER / PASS, ssh_proxy.py, Bitwarden-backed server credentials, read-only server.
---

# sshhh

Call the bundled launcher with its full path (it downloads the matching release binary for Linux, macOS or Windows on first run):

```sh
S="${CLAUDE_PLUGIN_ROOT}/scripts/sshhh"
```

Always pass `--json`. Errors are JSON on stderr (`{"error": code, "message": ...}`) and the tool's own failures exit 255; otherwise the exit code is the remote command's.

## Rules

- Never `cat`, print or paste a `.env` or key file. `$S list` shows servers without any secret value; `$S doctor` checks the setup.
- Never put a password in a command line. Credentials live in the `.env` or the vault, and `--root` supplies them.
- Become root only with `--root` (or `--sudo` / `--su`), never by typing `sudo` or `su` into the command.
- Move files with `put` / `get`, not shell redirection or `echo ... > file`.
- A refusal from the policy (`policy_denied`) is a decision of the server's owner. Report it; do not look for another way around it.
- A new host key is refused until the user has checked its fingerprint (see Host keys). Do not add `--accept-new` on your own.

## Running things

```sh
$S --json -s zeus uptime                     # one word: handed to the remote shell as written
$S --json -s zeus "df -h | tail -n 5"        # pipes and redirects need one quoted string
$S --json -s zeus systemctl status nginx     # several words: each is quoted for the remote shell
$S --json -s zeus --root -- ls /root         # remote command that starts with a subcommand name: use --
$S --json --all uptime                       # every server; or --on zeus,hera
$S --json -s zeus script deploy.sh arg1      # local script through `sh -s`, no temp file on the server
```

Single target, JSON: `{alias, rc, signal, stdout, stderr, duration_ms, escalation, reconnected, warnings}`. With `--all` / `--on` the result is an array, one entry per alias, a failed alias being `{alias, error, message}`. `rc` is the remote exit status. If the connection dies while a command runs the call fails with `connection_lost`; it is never run again automatically, so check the server state before retrying anything that is not idempotent.

Without `--json` a single target streams stdout and stderr live and exits with the remote status.

Standard input is not forwarded unless `--stdin` is given. `--timeout` (default 120 s) and `--connect-timeout` (default 15 s) are per call.

`--su` runs on a terminal: the remote stderr arrives merged into stdout and `--stdin` is refused.

## Files

```sh
$S --json -s zeus put ./app.tar.gz /srv/app/app.tar.gz --verify
$S --json -s zeus get /var/log/app.log ./app.log --verify
$S --json -s zeus get /etc/app/secret.key ./secret.key --private   # local file readable by the owner only
$S --json -s zeus ls /var/log
$S --json -s zeus cat /etc/hostname                                  # UTF-8 only; use get for binary
```

A transfer is written to a `.part` file and renamed when complete. `--verify` hashes both ends (SHA-256). Regular files only: directories are not transferred yet. File commands run over SFTP as the login user, so `--root`, `--sudo`, `--su` and `--stdin` are refused with a `usage` error; to read a root-owned file run `cat` under `--root` as a command (`$S --json --root -- cat /etc/x`).

## Servers: the `.env`

Aliases come from key prefixes: `ZEUS_HOST` makes the alias `zeus`. Bare keys (`HOST=...`) form the alias `default`. Lookup, later wins per key: `~/.config/sshhh/servers.env`, the nearest `.env` walking up from the current directory, `.local/.env`, then process environment variables prefixed `SSHHH_` (`SSHHH_ZEUS_PORT=2222`). `--env FILE` (or `SSHHH_ENV`) reads only that file. `DEFAULT=zeus` or `SSH_SERVER=zeus` picks the alias used without `-s`. The alias `ssh` is reserved.

```
DEFAULT=zeus
ZEUS_HOST=203.0.113.10
ZEUS_USER=deploy
ZEUS_KEY=~/.ssh/id_ed25519
ZEUS_SUDO_PASS=bw://zeus/sudo-password
HERA_HOST=198.51.100.7
HERA_USER=admin
HERA_PASS=bw://hera
HERA_ROOT_PASS=bw://hera/root-password
```

| Key | Meaning |
|---|---|
| `HOST`, `PORT` (22), `USER` (`root`) | where and as whom |
| `PASS` | login password |
| `KEY`, `KEY_PASS` | private key path (`~` allowed; OpenSSH and PuTTY `.ppk`, encrypted ones with `KEY_PASS`); a `<key>-cert.pub` next to it is used as the certificate |
| `AUTH` | order, from `key`, `password`, `kbdint`, `agent`; inferred from what is set when omitted |
| `SUDO_PASS` | `--root` uses `sudo -S`; passwordless sudo works without it |
| `ROOT_USER` (`root`), `ROOT_PASS` | `--root` uses `su` on a terminal |
| `ESCALATE` | `sudo`, `su` or `none` to force the mechanism |
| `VAULT` | `bw://ITEM`: fill the keys above that are not set from the item's custom fields named like the keys |

Passwords reach the remote side only on the channel's standard input or the terminal, never in a command line.

Values can be references: `bw://ITEM[/FIELD]` (Bitwarden CLI, cloud or self-hosted; FIELD is `password` by default, or `username`, `totp`, `notes`, `uri`, or a custom field name; percent-encode `/` in names), `env://NAME`, `file://PATH`. A failed lookup fails the call; nothing falls back to another value. Other vault schemes are refused with `not_implemented`.

Bitwarden: the `bw` CLI must be installed and logged in (`bw login`) on the server `bw config server` points to. The vault must be unlocked: `$S unlock bw` asks for the master password on the terminal once and keeps the session in the background process only. If a call fails with `vault_locked`, tell the user to run that command; do not try to unlock it yourself. Tool settings `BW_BIN`, `BW_APPDATA` and `BW_SERVER` (expected server URL) are honoured only in `~/.config/sshhh/servers.env`, an explicit `--env` file or `SSHHH_`-prefixed environment variables, never in a project `.env`.

Legacy key names (`SSH_HOST`, `IP`, `PASSWORD`, `CERT`, ...) still work and print a `deprecated_key` warning on stderr. `$S import <dir-or-file> --dry-run` shows the rewrite to the current names, without `--dry-run` it writes it.

## Read-only and policy

Set in the `.env` (or the vault item's custom fields), lists separated by `;`:

```
ZEUS_READONLY=true
ZEUS_ALLOW_COMMANDS=ls;cat;tail;systemctl status *;journalctl
ZEUS_DENY_COMMANDS=cat /etc/shadow
ZEUS_ALLOW_PATHS=/var/log;/srv/app
ZEUS_DENY_PATHS=/srv/app/.env;/home/*/.ssh
```

- The policy is enforced inside the tool for every command, script and file operation. No flag, later config layer or process variable can lower it: layers only tighten. `none` in an allow list allows nothing.
- `READONLY` refuses `put`, `--root`, output redirection and every command not in `ALLOW_COMMANDS`; with no allow list it refuses every command, while `get`, `ls` and `cat` (subject to the path lists) keep working. A catch-all entry such as `*` is rejected on a read-only server.
- An allow-list entry without spaces names a program; with spaces it matches the program and its arguments (`*` and `?` wildcards). Deny wins over allow. Wrappers (`sudo`, `env`, `nohup`, `timeout`, ...) and `sh -c` are looked through; every command of a pipeline or `;` / `&&` / `||` list is checked. Whatever cannot be parsed is denied: `$VAR`, `$(...)`, backticks, `( )` / `{ }` groups, a trailing `&`, here-documents.
- Paths are checked both as written and as resolved on the server (symlinks). A pattern covers the directory it names and everything below. While a path policy exists, path-like command arguments must be absolute (`/var/log/x`, not `logs/x` or `~/x`) and may not contain wildcards.
- Scripts and standard input are refused while any policy is set, because their content cannot be checked.
- Limits: an allowed program that can itself write or run other programs (`find -exec`, `awk`, an editor) is not detected; the policy is not a sandbox against a hostile login. Bare relative words are not known to be files. A different `--env` file is a different policy, so keep the real one out of anything the agent can edit. The vault part of a policy is read when the connection is made.

## Host keys

Host keys are strict and stored in `~/.config/sshhh/known_hosts`. An unknown key fails with its fingerprint. Show it to the user and ask them to compare it with the real server; then `$S trust zeus` (interactive confirmation) or `$S trust zeus --fingerprint SHA256:...`. A changed key is always refused; `$S forget zeus` removes the old one after the user has confirmed the change is expected. Host certificates are not supported.

## Background process and diagnostics

A per-user background process keeps one connection per server between calls and exits after 10 idle minutes. `$S daemon status` / `daemon stop` inspect and end it; `--no-daemon` uses a private connection for one call. Calls are recorded (time, alias, result, command hash, no command text unless `--audit-commands`) in `~/.local/state/sshhh/audit.jsonl`; if the log cannot be written the call does not run. Secrets configured for a server are masked in all output.

`$S doctor [alias]` checks the files' permissions and git exposure, vault state, host keys and that credentials resolve; it exits 1 on any failure. `$S list` shows each server, its auth order and policy, and where each secret comes from, never a value.

Language of human-readable messages: `SSHHH_LANG` (English only for now). JSON codes and error messages are always English.
