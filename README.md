# skills

Claude Code skills by [Version Two s.r.o.](https://www.versiontwo.sk)

```sh
claude plugin marketplace add version-two/skills
claude plugin install bazos@version-two
```

| Plugin | |
|---|---|
| `bazos` | Search bazos.sk / bazos.cz ads |
| `alza` | Search Alza products, filters, reviews |
| `sshhh` | Run commands and move files on servers over SSH, credentials from `.env` or Bitwarden |
| `subagent-build-orchestrator` | Run large builds with parallel subagents |
| `flaunch` | flaunch release CLI guidance |

`bazos`, `alza` and `sshhh` download their binary (Linux, macOS, Windows) from [releases](https://github.com/version-two/skills/releases) on first use. Sources in [`tools/`](tools).

MIT
