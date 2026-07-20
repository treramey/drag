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
verification, and the planned credential replacement.
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
reconfiguration replaces the connection credentials.
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

Pass an issue key followed by either a duration or a clock interval:

```bash
drag log ABC-123 1h15m
drag l ABC-123 11:35-14:20 yesterday -d "review"
drag log ABC-123 11.35-14.20 2026-07-14
drag log ABC-123 1h15m 2026-07-14 --start 09:30 --remaining-estimate 2h
drag log ABC-123 30m --start 12:00 --dry-run
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
drag --output json list --fields 'worklogs.id,worklogs.issueKey,worklogs.duration,pagination.next'
drag --output json list --limit 250 --page-limit 3
drag --output json list 2026-07-14 --continue-from '<pagination.next>'
drag --output json list --all-pages
drag --output ndjson list --limit 250 --page-limit 3
drag delete 123456 123457
drag delete --json '{"worklogIds":[123456,123457]}' --dry-run
printf '%s' '{"worklogIds":[123456,123457]}' | drag delete --json -

```

### Inspect API schemas

Schema output is JSON even in a human terminal. `drag schema` prints Drag's
complete machine-readable CLI contract. Pass a dotted Tempo component or
operation to inspect the official Tempo OpenAPI definition; add `--resolve-refs`
to inline local component references:

```bash
drag schema
drag schema tempo.Worklog --resolve-refs
drag schema tempo.worklogs.create --resolve-refs
```

Tempo schemas come only from
`https://apidocs.tempo.io/tempo-openapi.yaml` and are cached for 24 hours in
the platform cache directory. `DRAG_CACHE_DIR` overrides that location for
isolated automation.

Tempo API commands are generated from that same OpenAPI document at runtime.
Resource names come from OpenAPI tags, canonical methods come from operation
IDs, and short aliases such as `list` are added only when they are unambiguous:

```bash
drag tempo --help
drag tempo work-attributes list --params '{"limit":25}' --dry-run
drag --output json tempo work-attributes list --params '{"limit":25}'
drag tempo worklogs create --json '{"authorAccountId":"...","issueId":10001,"startDate":"2026-01-01","timeSpentSeconds":3600,"attributes":[{"key":"_Test_","value":"PS"},{"key":"_Worktype_","value":"Development"},{"key":"_PSTYPE_","value":"Consulting"}]}' --dry-run
```

`--params` must be a JSON object containing only path and query parameters
declared by the selected operation. The generated command validates required
parameters, JSON types, and enums before accessing Tempo. Operations with an
`application/json` request body accept inline JSON through `--json`; their
required fields and nested OpenAPI types are validated before access. A dry-run
prints the normalized method, URL, and body without calling the Tempo API.

`list` and its `ls` alias are read-only. With no date they select today in
Drag's configured local time zone; `--verbose` adds descriptions and Jira URLs
without changing machine field-selection behavior. When standard input, output,
and error are all terminals, resolved human output opens an interactive report
on stderr after retrieval completes. If any stream is redirected while human
output is explicit, Drag falls back to the completed plain-text report. With
the default `--output auto`, redirecting stdout selects JSON instead. On wide
terminals, one dashboard places the
selected-month calendar and month summary beside the selected date, focused
worklog table, and day summary. The calendar highlights today and the selected
date. Use `h`/`l` to
load the previous/next date and Up/Down or `k`/`j`
to navigate rows; overflowing tables scroll to keep the focused row visible.
Press `o` to open that row's resolved Jira browse URL in the local default
browser. This is an explicit local browser side effect: Drag makes no additional
Jira or Tempo API request and does not mutate either service, though the browser
may access the Jira URL. Success or failure is reported without closing the
report.
Press `q`, Escape, or Ctrl-C to close it. `--verbose`
shows the focused worklog's description and Jira URL below the responsive
table. Bounded reports label partial totals and empty retrieved segments.
While the interactive report is open, Drag checks the latest stable GitHub
release without blocking the interface. A newer version appears beneath the
current version in the header; timeouts, offline use, and check failures stay
silent.
Redirected human output, explicit JSON, and NDJSON remain non-interactive so
their existing fields and payloads are preserved.

For automation, pass `--output json` explicitly. This bypasses the interactive
report and preserves the existing list JSON contract regardless of terminal
attachment.

For automation, use `--fields` with a comma-delimited mask and request only the
data needed for the task. Select `date`; a whole `worklogs`, `schedule`, or
`pagination` subtree; or leaves such as `worklogs.id`,
`worklogs.interval.startTime`, `worklogs.issueKey`,
`schedule.dayLoggedDuration`, and `pagination.next`. Run
`drag --output json schema` for the complete allowed-field list. Parent fields
select their whole subtree. Duplicate fields, unknown paths, malformed paths,
and masks that combine a parent with one of its descendants fail before network
access. Omit `--fields` for the original complete JSON shape. Selected output
always uses canonical field ordering, independent of mask order.

Retrieval is bounded by default to at most 100 Tempo records and one page. Use
`--limit` (1–1000) and `--page-limit` (1–100) to choose another bounded segment.
When continuing, include `pagination.next` and `pagination.selectedDate` in a
narrow mask; pass `next` unchanged to `--continue-from` and repeat
`selectedDate` as `DATE`. Drag rejects continuations whose embedded month
differs from the selected date and never rewrites the continuation URL. The
token also restores its original record/page plan when those options are
omitted; explicitly supplied `--limit`, `--page-limit`, or `--all-pages` values
must match the token or fail before networking.

`--all-pages` is an explicit exhaustive mode and cannot be combined with an
explicit `--limit` or `--page-limit`. It still requests finite 100-record pages
and fails if Tempo provides more than 100 pages. Every result reports the
selected date and month range, effective limit, page limit, pages and records
retrieved, selected-day records returned, continuation, and whether traversal
is terminal. Schedule totals are calculated from the retrieved calendar-month
segment. `totalsComplete` is true only when the command started at the first
page and reached the terminal page; otherwise human output marks totals as
partial even when a resumed segment has no further continuation.

Use `--output ndjson` when records should be processed incrementally. Each
stdout line is one compact JSON object with a `kind` discriminator. Successful
streams contain zero or more `worklog` events, followed by one `summary` event
and one terminal `pagination` event:

```bash
drag --output ndjson list --fields \
  'worklogs.id,worklogs.issueKey,schedule.dayLoggedDuration,pagination.next,pagination.complete'
```

```jsonl
{"kind":"worklog","worklog":{"id":"123","issueKey":"ABC-1"}}
{"kind":"summary","date":"2026-07-14","schedule":{"dayLoggedDuration":"1h"}}
{"kind":"pagination","pagination":{"next":null,"complete":true}}
```

The complete event schemas are available through `drag --output json schema`.
`--fields` projects each event payload while retaining `kind`; if no worklog
field is selected, no worklog events or Jira lookups are performed. Tempo-only
fields such as `worklogs.id` also avoid unrelated Jira enrichment. Empty
results still emit the summary and pagination events. Pages are fetched and
records are flushed incrementally, so records already written remain valid if
a later Tempo page or Jira enrichment fails. On failure Drag stops without a
terminal pagination event, writes the normal structured error envelope to
stderr, and exits non-zero. A consumer closing stdout early is treated as a
successful end to the stream. NDJSON is explicit, supported only by `list`,
and preserves the same record/page limits, all-pages ceiling, continuation
validation, and field-selection rules as regular JSON.

`delete` accepts ordered numeric IDs either positionally or in a camel-case
JSON object through `--json`, with `-` reading from stdin. JSON payloads use
`worklogIds`, for example `{"worklogIds":[123456,123457]}`. It processes IDs
sequentially in the supplied order and stops on the first error. Batch deletion
is not atomic: worklogs deleted before a later failure remain deleted. Use
`--dry-run` to perform the same ordered reads and preview every target without
deleting it.

### JSON and raw input

Output defaults to human text in a terminal and JSON when redirected. Pin the
contract explicitly in automation:

```bash
drag --output json list --fields 'worklogs.issueKey,worklogs.duration,pagination.next' | jq
drag --output ndjson list --fields 'worklogs.issueKey,worklogs.duration,pagination.next'
drag --output json schema
printf '%s' '{"issueKey":"ABC-1","durationOrInterval":"30m"}' \
  | drag --output json log --json - --dry-run
```

Raw log input uses camel-case fields: `issueKey`,
`durationOrInterval`, `when`, `description`, `start`, and
`remainingEstimate`. Unknown fields are rejected. `--json` cannot be combined
with positional log arguments or their corresponding flags.

Successful JSON uses `{"ok":true,"data":...}`. Errors go to stderr as
`{"ok":false,"error":{"code":"...","message":"..."}}`.
`--debug` writes redacted request diagnostics only in human output mode; JSON
and NDJSON output stay machine-readable.

`drag --output json schema` emits the versioned CLI contract. Schema version 4
includes the installed CLI version and every command, nested subcommand,
shortcut, and hidden compatibility form. Arguments report their types,
cardinality, defaults, enums, conditional requirements, and conflicts. Each
command also describes its JSON success data, possible structured error codes,
side effects, network access, and dry-run behavior. The `--json` arguments for
log and worklog deletion contain nested JSON Schemas generated
from the same serde input types used at runtime, while command and option
metadata is read from Clap's command model. The schema command also documents
its local-contract, Tempo component, and Tempo operation result variants.

| Exit | Meaning |
|---:|---|
| `0` | Success |
| `1` | Config, network, server, or I/O failure |
| `2` | Invalid command, input, date, or duration |

## AI agent skills

Drag ships portable Agent Skills generated from the CLI contract and Tempo's
official OpenAPI document. Install every skill from the repository:

```bash
npx skills add https://github.com/treramey/drag
```

Or install one task skill:

```bash
npx skills add https://github.com/treramey/drag/tree/main/skills/drag-log
npx skills add https://github.com/treramey/drag/tree/main/skills/drag-tempo
```

The local skills document `log`, `list`, and `delete` directly from Drag's Clap
and `drag schema` metadata. `drag-tempo` includes resource references generated
from the live Tempo OpenAPI operation catalog. See the complete
[skills index](docs/skills.md).

Maintainers can regenerate repository-controlled skills without network access,
refresh only Tempo's external catalog, or refresh both:

```bash
drag generate-skills --scope local --force
drag generate-skills --scope tempo --force
drag generate-skills --scope all --force
```

`--force` is required when replacing existing generated skill directories.

## Backward compatibility

The binary is `drag`; shortcuts `l`, `ls`, and `d` remain available.

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
