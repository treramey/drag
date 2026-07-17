# Drag

A fast, scriptable CLI for logging time in
[Tempo Cloud](https://tempo.io), with practical command shortcuts, structured
output, dry runs, safer HTTP behavior, and cross-platform binaries.

> The rewrite is currently pre-1.0. Exercise `--dry-run` and verify behavior in
> a non-production Tempo account before replacing an existing installation.

## Features

- Log work using durations (`1h15m`) or intervals (`11-12:30`).
- List daily worklogs with monthly required/logged totals.
- Delete one or several worklogs.
- Store aliases such as `lunch => ABC-123`.
- Read the legacy TypeScript CLI's config format.
- Return readable terminal output or consistent JSON for scripts and agents.
- Preview mutations with `--dry-run`.

## Installation

Download a checksummed native archive from
[GitHub Releases](https://github.com/treramey/drag/releases), or install the
npm bootstrap package:

```bash
npm install --global @treramey/drag
```

The npm package downloads the matching release binary and verifies its SHA-256
checksum. A Nix flake is also available with `nix run github:treramey/drag`.
When the Homebrew tap is configured, use `brew install treramey/tap/drag`.

### Install from source

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

Preview unattended setup without network access or configuration writes:

```bash
drag --output json setup --from-env --dry-run
```

The plan reports completed local validation, planned read-only Jira and Tempo
verification, and the planned credential replacement while preserving aliases.
Add `--verify` to perform the read-only connection checks during the dry-run;
the configuration is still not written. Tokens are accepted only through the
four environment variables and never through command arguments or JSON.

`ATLASSIAN_HOST` may be a bare hostname or any HTTPS URL on the Jira site. The
runtime `TEMPO_ACCOUNT_ID` override remains supported for compatibility, but
setup does not require or trust it; verified setup always uses the account ID
returned by Jira. Headless setup never prompts or opens a browser. Use
`--config <PATH>` to select another config file.

### Setup safety

Setup reads the current configuration before asking for credentials and writes
once, after both read-only connection checks succeed. Cancellation and failed
validation or verification leave the existing file unchanged. A successful
reconfiguration replaces only connection credentials, preserving aliases.
Config files use user-only permissions on Unix. Tokens are never
printed or included in human output, JSON, debug diagnostics, or errors.

### Check connections

`drag doctor` reports local configuration and runtime diagnostics without
network access. Run `drag doctor --remote` to repeat the same read-only Jira
and Tempo connection checks used by setup. Remote results are reported for
both services when possible. Remote request failures exit with status 1;
missing or invalid connection settings exit with status 2. Doctor never
changes the configuration.

## Usage

### Log work

Pass an issue key or configured alias followed by either a duration or a clock
interval:

```bash
drag log ABC-123 1h15m
drag l ABC-123 11:35-14:20 yesterday -d "review"
drag log ABC-123 11.35-14.20 2026-07-14
drag log lunch 1h15m 2026-07-14 --start 09:30 --remaining-estimate 2h
drag log lunch 30m --start 12:00 --dry-run
```

Durations accept minutes, hours, or both, such as `15m`, `1h`, and `1h15m`.
Intervals accept whole hours and either colon or dot clock notation, including
`11-14`, `11-14:30`, `11:35-14:20`, and `11.35-14.20`. An interval supplies
its own start time. If its end is at or before its start, Drag treats the end
as the following local day.

`WHEN` defaults to today in Drag's configured local time zone. It accepts
`YYYY-MM-DD`, `y`, `yesterday`, `t±N`, and `today±N`. For duration input,
`--start`/`-s` sets the start in `HH:mm` form. Without it, today's worklog uses
the current local time; a selected date uses the start of that day.
`--description`/`-d` adds worklog text, and `--remaining-estimate`/`-r` accepts
a duration such as `2h`.

Live logging reads Jira once to resolve the issue ID, then creates one Tempo
worklog. Drag does not retry the create request automatically. Use `--dry-run`
to validate the command and print the normalized request without contacting
Jira or Tempo.

### Other commands

```bash
# Worklogs
drag list
drag ls 2026-07-14 --verbose
drag --output json list --limit 250 --page-limit 3
drag --output json list --continue-from 'https://api.tempo.io/4/worklogs?...'
drag --output json list --all-pages
drag delete 123456 123457
drag delete --json '{"worklogIds":[123456,123457]}' --dry-run
printf '%s' '{"worklogIds":[123456,123457]}' | drag delete --json -

# Aliases (both modern and original colon forms work)
drag alias set lunch ABC-123
drag alias set lunch ABC-456 --dry-run
printf '%s' '{"alias":"lunch","issueKey":"ABC-123"}' \
  | drag alias set --json - --dry-run
drag alias list
drag alias delete lunch
drag alias:set lunch ABC-123

```

`list` and its `ls` alias are read-only. With no date they select today in
Drag's configured local time zone; `--verbose` adds descriptions and Jira URLs
to human output without changing the JSON data shape. Retrieval is bounded by
default to at most 100 Tempo records and one page. Use `--limit` (1–1000) and
`--page-limit` (1–100) to choose another bounded segment. JSON results include
`pagination.next`; pass that exact, opaque value to `--continue-from` to
retrieve the next segment deterministically.

`--all-pages` is an explicit exhaustive mode and cannot be combined with an
explicit `--limit` or `--page-limit`. It still requests finite 100-record pages
and fails if Tempo provides more than 100 pages. Every result reports the
effective limit, page limit, pages and records retrieved, selected-day records
returned, continuation, and whether traversal is complete. Schedule totals
are calculated from the retrieved calendar-month segment. When `complete` is
false, those totals may be partial and human output says so.

`delete` accepts ordered numeric IDs either positionally or in a camel-case
JSON object through `--json`, with `-` reading from stdin. JSON payloads use
`worklogIds`, for example `{"worklogIds":[123456,123457]}`. It processes IDs
sequentially in the supplied order and stops on the first error. Batch deletion
is not atomic: worklogs deleted before a later failure remain deleted. Use
`--dry-run` to perform the same ordered reads and preview every target without
deleting it.

Alias set and delete operations accept either their positional arguments or a
camel-case JSON document through `--json`, with `-` reading from stdin. Set
payloads contain `alias` and `issueKey`; delete payloads contain `alias`.
`--dry-run` reports an `action` of `create`, `replace`, `delete`, or
`unchanged` without rewriting the config file. Live execution uses the same
normalized change plan and skips config writes for unchanged operations.

### JSON and raw input

Output defaults to human text in a terminal and JSON when redirected. Pin the
contract explicitly in automation:

```bash
drag --output json list | jq
drag --output json schema
printf '%s' '{"issueKeyOrAlias":"ABC-1","durationOrInterval":"30m"}' \
  | drag --output json log --json - --dry-run
```

Raw log input uses camel-case fields: `issueKeyOrAlias`,
`durationOrInterval`, `when`, `description`, `start`, and
`remainingEstimate`. Unknown fields are rejected. `--json` cannot be combined
with positional log arguments or their corresponding flags. Alias JSON follows
the same unknown-field and convenience-argument conflict rules.

Successful JSON uses `{"ok":true,"data":...}`. Errors go to stderr as
`{"ok":false,"error":{"code":"...","message":"..."}}`.
`--debug` writes redacted request diagnostics only in human output mode; JSON
output stays machine-readable.

`drag --output json schema` emits the versioned CLI contract. Schema version 2
includes the installed CLI version and every command, nested subcommand,
shortcut, and hidden compatibility form. Arguments report their types,
cardinality, defaults, enums, conditional requirements, and conflicts. Each
command also describes its JSON success data, possible structured error codes,
side effects, network access, and dry-run behavior. The `--json` arguments for
log, worklog deletion, and alias mutations contain nested JSON Schemas generated
from the same serde input types used at runtime, while command and option
metadata is read from Clap's command model.

| Exit | Meaning |
|---:|---|
| `0` | Success |
| `1` | Config, network, server, or I/O failure |
| `2` | Invalid command, input, date, or duration |

## Backward compatibility

The binary is `drag`; shortcuts `l`, `ls`, and `d` remain available. Original
alias command names such as `alias:set` and `alias:list` are accepted.
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
