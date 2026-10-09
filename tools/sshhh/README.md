# sshhh

Quiet, fast SSH for agents and scripts. One binary reads server definitions from a `.env`, keeps connections open between calls, runs commands with `sudo` or `su`, moves files over SFTP, takes credentials from Bitwarden and answers in JSON. It never prints a secret.

Part of [version-two/skills](https://github.com/version-two/skills); the Claude Code skill lives in [`plugins/sshhh`](../../plugins/sshhh).

```sh
sshhh --json -s zeus uptime
sshhh --json --all "df -h /"
sshhh --json -s zeus --root -- systemctl restart nginx
sshhh -s zeus put ./app.tar.gz /srv/app/app.tar.gz --verify
```

## Servers

```
DEFAULT=zeus
ZEUS_HOST=203.0.113.10
ZEUS_USER=deploy
ZEUS_KEY=~/.ssh/id_ed25519
ZEUS_SUDO_PASS=bw://zeus/sudo-password
```

Files are read from `~/.config/sshhh/servers.env`, the nearest `.env` walking up from the working directory and `.local/.env`; `SSHHH_`-prefixed environment variables override them and `--env FILE` replaces all of it. Keys: `HOST`, `PORT`, `USER`, `PASS`, `KEY`, `KEY_PASS`, `AUTH`, `ROOT_USER`, `ROOT_PASS`, `SUDO_PASS`, `ESCALATE`, `VAULT`, and the policy keys below.

Authentication: password, keyboard-interactive, OpenSSH and PuTTY (`.ppk`) keys with or without passphrase, OpenSSH user certificates, ssh-agent.

Values may be references: `bw://ITEM[/FIELD]` (Bitwarden CLI, cloud or self-hosted), `env://NAME`, `file://PATH`. A failed lookup fails the call.

## Commands

| | |
|---|---|
| `sshhh [flags] <command...>` | run a command; the remote exit status is the exit status |
| `script <file\|->` | run a local script through `sh -s` |
| `put`, `get` | SFTP, `--verify` (SHA-256), `--private` |
| `ls`, `cat` | inspect remote files |
| `list`, `doctor` | configured servers without secrets; checks of files, vault, host keys |
| `trust`, `forget` | host key management (strict by default) |
| `daemon status\|stop` | the background process that holds the connections |
| `unlock bw` | unlock the Bitwarden vault, keep the session in the background process |
| `import` | rewrite legacy key names (`SSH_HOST`, `PASSWORD`, ...) |

Tool errors exit 255 and print `{"error": code, "message": ...}` on stderr. `--json` output is described in the skill.

## Read-only and policy

`READONLY=true`, `ALLOW_COMMANDS`, `DENY_COMMANDS`, `ALLOW_PATHS`, `DENY_PATHS` (lists separated by `;`) are enforced by the tool for every command, script and file operation. Layers can only tighten. They are not a sandbox: see [docs/DESIGN.md](docs/DESIGN.md).

## Build

```sh
cargo build --release -p sshhh-cli     # from the tools/ directory
cargo test -p sshhh-cli
```

Needs a native `bw` binary for the `bw://` provider. Release binaries are built for Windows, Linux and macOS; the test suite has been run on Windows and Linux.

[`plan.md`](plan.md) tracks what is built and what is deferred.

MIT, Version Two s.r.o.
