# Drag

A fast, scriptable CLI for logging and tracking time in
[Tempo Cloud](https://tempo.io), with practical command shortcuts, structured
output, dry runs, safer HTTP behavior, and cross-platform binaries.

> The rewrite is currently pre-1.0. Exercise `--dry-run` and verify behavior in
> a non-production Tempo account before replacing an existing installation.

## Features

- Log work using durations (`1h15m`) or intervals (`11-12:30`).
- List daily worklogs with monthly required/logged totals.
- Delete one or several worklogs.
- Store aliases such as `lunch => ABC-123`.
- Start, pause, resume, inspect, and stop persistent local trackers.
- Read the legacy TypeScript CLI's config and tracker format.
- Return readable terminal output or consistent JSON for scripts and agents.
- Preview mutations with `--dry-run`.

## Install from source

```bash
cargo install --path crates/drag-cli
drag --version
```

Version tags also produce checksummed Linux, macOS, and Windows binaries in
GitHub Releases.

## Configuration

### Interactive setup

Run setup in a terminal:

```bash
drag setup
```

The Ratatui wizard requires terminal-capable standard input and standard error
and proceeds through **Jira account details**, **Atlassian API token**, **Tempo
account**, and **Review & save**. Use Tab and Shift-Tab to move between fields
and actions, Enter to continue, and Escape to return to the previous stage.
Escape cancels from Jira account details; Ctrl-C cancels from any stage,
including during a connection check. Cancellation never saves configuration.
On Review & save, press J to edit Jira or T to edit Tempo before saving. For
Jira, enter either a bare hostname such as
`yourcompany.atlassian.net` or any HTTPS URL from that Jira site. Setup
verifies Jira and Tempo without blocking keyboard input, then shows a
non-secret review before anything is saved.

Resize events preserve entered values and redraw the wizard. Terminals smaller
than 84 columns by 28 rows show a resize instruction until enough space is
available. Focus, pending, connected, warning, and error states use text and
symbols as well as color. Connection checks use a compact spinner, successful
checks resolve with a short status reveal, and the review diagram carries a
brief signal from Jira to Tempo without moving the credential cards.
Focused input borders carry a subtle color cycle around the field; action
focus remains immediate and static.

Set `DRAG_REDUCED_MOTION=1` to replace glyph motion with short color-only
transitions and a static pending indicator. Keyboard focus changes are always
immediate.

Typed and pasted tokens are masked. Each connection stage opens the relevant
token settings in your default browser. Use `drag setup --no-open` to keep the
URLs in the terminal without launching a browser, such as over SSH. A browser
error is only a warning; use the displayed URL and continue.

When reconfiguring, the current Jira site and email are offered as defaults.
Each saved token can be retained without displaying or copying it. If Jira or
Tempo rejects credentials, setup keeps the current stage available for another
attempt. Editing verified Jira details requires both connections to be checked
again; replacing only the Tempo token keeps Jira connected. Nothing changes on
disk until the final Save action.

The wizard renders on standard error and restores raw mode, the alternate
screen, cursor visibility, and bracketed paste before printing its result.
Consequently, `drag --output json setup` keeps successful JSON on standard
output free of terminal control sequences and onboarding text.

### Headless setup

For headless use, provide the four connection variables and run `drag setup
--from-env`. Setup verifies Jira and Tempo with read-only requests, derives the
Atlassian account ID from Jira, and saves only after both checks succeed:

```bash
export TEMPO_TOKEN=...
export ATLASSIAN_EMAIL=you@example.com
export ATLASSIAN_TOKEN=...
export ATLASSIAN_HOST=https://yourcompany.atlassian.net/jira/software
drag setup --from-env
```

`ATLASSIAN_HOST` may be a bare hostname or any HTTPS URL on the Jira site. The
runtime `TEMPO_ACCOUNT_ID` override remains supported for compatibility, but
setup does not require or trust it; verified setup always uses the account ID
returned by Jira. Headless setup never prompts or opens a browser. Use
`--config <PATH>` to select another config file.

### Setup safety

Setup reads the current configuration before asking for credentials and writes
once, after both read-only connection checks succeed. Cancellation and failed
validation or verification leave the existing file unchanged. A successful
reconfiguration replaces only connection credentials, preserving aliases and
trackers. Config files use user-only permissions on Unix. Tokens are never
printed or included in human output, JSON, debug diagnostics, or errors.

### Check connections

`drag doctor` reports local configuration and runtime diagnostics without
network access. Run `drag doctor --remote` to repeat the same read-only Jira
and Tempo connection checks used by setup. Remote results are reported for
both services when possible. Remote request failures exit with status 1;
missing or invalid connection settings exit with status 2. Doctor never
changes the configuration.

## Usage

```bash
# Worklogs
drag log ABC-123 1h15m
drag l ABC-123 11-12:30 yesterday -d "review"
drag log lunch 30m --start 12:00 --dry-run
drag list
drag ls 2026-07-14 --verbose
drag delete 123456 123457

# Aliases (both modern and original colon forms work)
drag alias set lunch ABC-123
drag alias list
drag alias:set lunch ABC-123

# Trackers
drag tracker start ABC-123 -d "implementation"
drag pause ABC-123
drag resume ABC-123
drag tracker list
drag stop ABC-123 --dry-run
drag stop ABC-123
```

Accepted date selectors are `YYYY-MM-DD`, `y`, `yesterday`, `t±N`, and
`today±N`. Intervals that end before their start cross midnight.

### JSON and raw input

Output defaults to human text in a terminal and JSON when redirected. Pin the
contract explicitly in automation:

```bash
drag --output json list | jq
drag --output json schema
printf '%s' '{"issueKeyOrAlias":"ABC-1","durationOrInterval":"30m"}' \
  | drag --output json log --json - --dry-run
```

Successful JSON uses `{"ok":true,"data":...}`. Errors go to stderr as
`{"ok":false,"error":{"code":"...","message":"..."}}`.

| Exit | Meaning |
|---:|---|
| `0` | Success |
| `1` | Config, network, server, or I/O failure |
| `2` | Invalid command, input, date, or duration |

## Backward compatibility

The binary is `drag`; shortcuts `l`, `ls`, `d`, `start`, `pause`,
`resume`, and `stop` remain available. Original command names such as
`alias:set`, `alias:list`, `tracker:start`, and `tracker:list` are accepted.
The config reader supports the TypeScript `Map` JSON representation and writes
the same representation so rollback remains possible.

Behavioral fixes are intentional: malformed config is reported instead of
silently discarded, issue/account URL segments are validated, and Tempo
pagination cannot redirect an authenticated request away from
`api.tempo.io`.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo install cargo-deny
cargo deny check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) and
[docs/architecture.md](docs/architecture.md).

## Acknowledgements

Credit to Szymon Kozak for the original
[Tempomat](https://github.com/szymonkozak/tempomat) project. Both projects are
MIT licensed. See [LICENSE](LICENSE).
