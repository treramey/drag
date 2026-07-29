<h1 align="center">Drag</h1>

**Log time in Tempo Cloud from the command line.** Fast shortcuts for people,
structured output for scripts and AI agents.

<p>
  <a href="https://github.com/treramey/drag/releases"><img src="https://img.shields.io/github/v/release/treramey/drag" alt="Latest release"></a>
  <a href="https://github.com/treramey/drag/actions"><img src="https://img.shields.io/github/actions/workflow/status/treramey/drag/ci.yml?branch=main&label=CI" alt="CI status"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/treramey/drag" alt="MIT license"></a>
</p>

> [!IMPORTANT]
> Drag is pre-1.0. Use `--dry-run` and test with a non-production Tempo account
> before replacing an existing installation.

## Install

Download a checksummed binary from
[GitHub Releases](https://github.com/treramey/drag/releases), or install with
npm:

```bash
npm install --global @treramey/drag
```

Other options:

```bash
nix run github:treramey/drag
brew install treramey/tap/drag
```

To build from this repository:

```bash
cargo install --path crates/drag-cli
cargo install --path crates/drag-companion
```

## Quick start

Connect Drag to Jira and Tempo:

```bash
drag setup
```

Then log and inspect your time:

```bash
drag log ABC-123 1h15m
drag log ABC-123 11:35-14:20 yesterday -d "Code review"
drag list
drag list yesterday
```

Use `--dry-run` to preview a change without sending it:

```bash
drag log ABC-123 30m --start 12:00 --dry-run
drag delete 123456 --dry-run
```

## Commands

| Command | Shortcut | What it does |
|---|---|---|
| `drag log` | `drag l` | Add a worklog from a duration or clock interval |
| `drag list` | `drag ls` | List worklogs for a date |
| `drag delete` | `drag d` | Delete one or more worklogs |
| `drag setup` | | Connect and verify Jira and Tempo |
| `drag doctor` | | Check configuration and connections |
| `drag tracking` | | Configure, run, review, pause, and inspect automatic tracking |
| `drag tempo` | | Call the Tempo API from generated commands |
| `drag schema` | | Inspect Drag or Tempo schemas |

Run `drag <command> --help` for every option.

Automatic tracking is a Drag capability backed by a separately packaged,
versioned `drag-tracking` process. Configure approved local sources, a weekday
schedule, and a submission policy, then inspect or run it through Drag:

Interactive `drag setup` offers this tracking wizard only after Jira and Tempo
have both connected. Declining or cancelling applies no tracking choices. The
same wizard opens from `drag tracking setup` in a terminal and explains local
evidence access, network use, scheduler and hook installation, and the separate
permission required for automatic worklog submission. Non-terminal setup keeps
the existing flag-driven behavior.

```bash
drag tracking setup --mode draft
# Configure the offline proposal adapter when collected evidence should become proposals:
drag tracking setup --provider-fixture ./provider.json
drag tracking status
drag tracking run today
drag tracking review today
drag tracking sources list
drag tracking sources configure --repo ~/work/project
drag tracking sources configure --ics ~/calendar.ics
drag tracking sources test today
drag tracking schedule show
drag --output json tracking status
```

Draft mode cannot submit worklogs. Review mode requires an approval bound to
the current proposal-set digest. Automatic mode requires separate setup-time
authorization plus every runtime rollout, duplicate, uncertainty, and kill
switch gate. Scheduler files, Claude hooks, and automatic submission are
authorized independently. Tracking state lives under `~/.drag/tracking`; the
first run atomically migrates a legacy `.drag-companion` store when only that
store exists.

Source configuration validates each selected local repository and calendar
before saving it and leaves other source kinds unchanged. Claude Code can be
selected with `sources configure --claude-code` after its separately authorized
hooks are installed. Source tests cap observations, return redacted references
instead of paths or evidence content, and never persist evidence or generate or
submit worklogs.

Complete runs stop before audit when an enabled source fails. Evidence-bearing
runs also stop before audit until a schema-valid proposal provider fixture is
configured. Re-running setup preserves source and scheduler settings that were
not supplied again.

Low-level diagnostics, replay, scheduler entrypoints, and recovery tools live
under the hidden `drag-tracking internal` surface. Normal users and agents
should use only the intent-level `drag tracking` commands above. The legacy
direct pipeline spellings on `drag-tracking` delegate to their exact
`drag-tracking internal <command>` replacements and retain the same safety
gates during the compatibility window.

The deprecated `drag-companion` executable remains available through the 0.9.x
minor-release line as a compatibility shim. It delegates without changing
structured stdout or mutation behavior. Both the shim and legacy direct
pipeline spellings are removed in 0.10.0; migrate scripts to `drag tracking`
for intent workflows or `drag-tracking internal` for operator recovery before
upgrading. Rollback remains possible by pausing tracking, restoring the legacy
state directory as described in `docs/architecture.md`, and reinstalling 0.9.x.

When `drag list` opens an interactive terminal report, use Left/Right Arrow or
`h`/`l` to change days, Up/Down Arrow or `k`/`j` to move between worklogs, and
`o` to open the focused worklog's Jira issue. Press `q`, Escape, or Ctrl-C to
close the report. Adding time remains a separate `drag log` workflow.

### Dates and durations

Durations use forms such as `15m`, `1h`, and `1h15m`. Clock intervals can use
colons or dots:

```bash
drag log ABC-123 11-14
drag log ABC-123 11:35-14:20
drag log ABC-123 11.35-14.20 2026-07-14
```

Dates accept `YYYY-MM-DD`, `today`, `yesterday`, `y`, `t-1`, and similar
offsets. The date defaults to today in Drag's local time zone.

### Worklog details

```bash
drag log ABC-123 1h \
  --description "Code review" \
  --remaining-estimate 2h \
  --attr _Worktype_=Development
```

Repeat `--attr KEY=VALUE` for Tempo work attributes.

## Configuration

`drag setup` opens an interactive wizard. It verifies Jira and Tempo before
writing the configuration file. Tokens are masked and are never included in
output, errors, or debug logs.

For a server or CI environment, provide the connection settings through the
environment:

```bash
export ATLASSIAN_HOST=https://yourcompany.atlassian.net
export ATLASSIAN_EMAIL=you@example.com
export ATLASSIAN_TOKEN=...
export TEMPO_TOKEN=...

drag setup --from-env
```

Headless setup does not prompt or open a browser. Preview it without network
access or file changes with:

```bash
drag --output json setup --from-env --dry-run
```

Check the saved configuration locally with `drag doctor`, or verify both
services with `drag doctor --remote`.

## JSON and automation

Drag prints readable text in a terminal and JSON when output is redirected.
Pin the format in scripts:

```bash
drag --output json list
drag --output json list --fields 'worklogs.id,worklogs.issueKey,worklogs.duration,pagination.next'
drag --output ndjson list --all-pages
```

Successful JSON uses `{"ok":true,"data":...}`. Errors go to stderr as
`{"ok":false,"error":{"code":"...","message":"..."}}`.

Commands that accept a body can read JSON directly or from stdin:

```bash
printf '%s' '{"issueKey":"ABC-1","durationOrInterval":"30m"}' \
  | drag --output json log --json - --dry-run
```

Inspect the complete machine-readable CLI contract with:

```bash
drag --output json schema
```

## Tempo API

Drag builds Tempo resource commands from Tempo's official OpenAPI document at
runtime:

```bash
drag tempo --help
drag tempo work-attributes list --params '{"limit":25}'
drag tempo worklogs create --json '{"authorAccountId":"...","issueId":10001,"startDate":"2026-01-01","timeSpentSeconds":3600}' --dry-run
```

Use `drag schema tempo.<resource>.<operation>` to inspect an operation before
calling it:

```bash
drag schema tempo.worklogs.create --resolve-refs
```

## AI agent skills

Drag includes portable Agent Skills for its commands, common worklog tasks, and
the Tempo API.

```bash
# Install all skills
npx skills add https://github.com/treramey/drag

# Install one skill
npx skills add https://github.com/treramey/drag/tree/main/skills/drag-log
```

See the [skills index](docs/skills.md) for the full list.

## Exit codes

| Exit | Meaning |
|---:|---|
| `0` | Success |
| `1` | Configuration, network, server, or I/O failure |
| `2` | Invalid command or input |

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
```

See [CONTRIBUTING.md](CONTRIBUTING.md) and
[docs/architecture.md](docs/architecture.md).

## License

[MIT](LICENSE). Drag is based on Szymon Kozak's
[Tempomat](https://github.com/szymonkozak/tempomat).
