# sshhh – design notes

What the tool does, the decisions behind it and where its limits are. The plan and open items are in [`../plan.md`](../plan.md).

## Goals

Replace per-project SSH helper scripts with one tool that is safe to hand to an agent: strict host keys, passwords never in a command line, the real remote exit status, one connection reused across calls, machine-readable output, and a way for the server's owner to restrict what may be done.

## Configuration

- Servers are defined by key prefix: `ZEUS_HOST` is the alias `zeus` (longest-suffix match on the key name); bare keys form `default`; the alias `ssh` is reserved.
- Layers, later wins per key: `~/.config/sshhh/servers.env`, the nearest `.env` walking up from the working directory, `.local/.env`, process variables prefixed `SSHHH_`. `--env FILE` / `SSHHH_ENV` is exclusive: nothing else is read.
- Empty values count as unset. Two spellings of one key with different values are an error. Legacy spellings (`SSH_HOST`, `IP`, `PASSWORD`, `CERT`, ...) still work and print a `deprecated_key` warning; `import` rewrites them.
- Settings that can launch a binary or choose a server (`BW_BIN`, `BW_APPDATA`, `BW_SERVER`) are honoured only from the global file, an explicit `--env` file or the process environment, never from a project `.env`.
- Values may be references (`bw://`, `env://`, `file://`). Only known scheme names are references, so `C:\key` or `https://...` stay literal. Other vault schemes are recognised and refused as `not_implemented`.

## Credentials from Bitwarden

The tool drives the native `bw` CLI (non-interactive, `--raw`, its own `BITWARDENCLI_APPDATA_DIR` when configured), so cloud and self-hosted servers work the same and the vault format stays Bitwarden's business. An item is fetched once per call however many fields are read. `BW_SERVER` is an expectation: a mismatch with `bw config server` fails with `bw_wrong_server` instead of asking the wrong server. A locked vault fails with `vault_locked`; `unlock bw` runs `bw unlock` on the terminal and keeps the session key in the background process's memory only. `VAULT=bw://ITEM` fills keys that the `.env` leaves unset from the item's custom fields named like the keys, and the item may also carry policy keys.

No fallbacks: a failed lookup fails the call; it never substitutes another value, another provider or a cached copy.

## Connection handling

- One russh session per alias and resolved configuration, with keepalive. The background process is started on demand, one per user, and exits after 10 idle minutes.
- IPC is a Unix socket (mode 0600 in a 0700 directory) or a Windows named pipe whose ACL names the user's SID, plus a token file compared in constant time. The process starts with a cleared environment and an allow-list of variables.
- If a held session is found dead before a command is sent it is replaced and the reconnect is reported (`reconnected`). If the connection dies during a command the call fails with `connection_lost`; the command is never run again. The client never falls back to an in-process connection unless `--no-daemon` is given.

## Escalation

- `sudo`: `sudo -S -p ''` with the password on the channel's standard input, streams stay separate.
- `su`: on a pseudo-terminal, waiting for the password prompt, failing on a missing prompt or rejected password; stderr is merged into stdout because the terminal has one stream.
- Chosen from what is configured (`SUDO_PASS`, `ROOT_PASS`), or forced with `ESCALATE=su|sudo|none`. Passwords never appear in a command string.

## Files

SFTP as the login user. A transfer goes to a `.part` file and is renamed when complete; `--verify` hashes both ends with SHA-256; `--private` creates the local file readable by the owner only (`0600`, or a single-SID ACL on Windows). Only regular files are transferred.

## Host keys

Strict by default, stored in `~/.config/sshhh/known_hosts`. An unknown key fails with its fingerprint; `trust` stores it after an interactive confirmation or a matching `--fingerprint`. A changed key is always refused. Host certificates are not supported.

## Output and logs

Configured secret values (4 characters or longer) are masked in all output, including terminal echo. The audit log (`~/.local/state/sshhh/audit.jsonl`) holds time, alias, user, mechanism, result, duration and a command hash, and the command text only with `--audit-commands`; if it cannot be written the call does not run.

## Read-only and policy

Keys per server: `READONLY`, `ALLOW_COMMANDS`, `DENY_COMMANDS`, `ALLOW_PATHS`, `DENY_PATHS`; lists use `;`.

- Every layer (files, process variables, vault item) can only tighten; nothing lowers `READONLY` or widens a list. A different `--env` file is a different policy.
- Commands are parsed into words (quotes, pipelines, `;`, `&&`, `||`, `sh -c`, wrappers such as `sudo`, `env`, `timeout`), and each simple command is checked. Whatever cannot be parsed is denied: variable and command substitution, backticks, grouping (`( )`, `{ }`), background `&`, here-documents and process substitution. An entry without whitespace names a program, one with whitespace matches program and arguments with `*` and `?`. Deny wins.
- Paths are checked as written and as resolved on the server, so a symlink cannot lead out of an allowed directory. A pattern covers the directory it names and everything below. While a path policy exists, path-like arguments must be absolute and free of wildcards.
- `READONLY` additionally refuses uploads, escalation and output redirection, and refuses every command unless `ALLOW_COMMANDS` lists it; a catch-all entry is rejected.
- Standard input (scripts, `--stdin`) is refused while any policy is set, since its content cannot be checked.

Limits: an allowed program that can itself write or run other programs is not detected; this is a guard rail for agents, not a sandbox against a hostile login. Bare relative words are not known to be files. The vault part of a policy is read when the connection is made.

## Output contract

`--json`: single target `{alias, rc, signal, stdout, stderr, duration_ms, escalation, reconnected, warnings}`; fan-out an array with `{alias, error, message}` for a failed alias. Errors: `{"error": code, "message": ...}` on stderr. Exit status: the remote status; 255 for the tool's own failures; `doctor` exits 1 on any failed check.

Human-readable CLI text comes from `locales/en.json` (`SSHHH_LANG`); error messages and JSON codes stay English.
