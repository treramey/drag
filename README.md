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

Interactive setup stores credentials in a local config file with user-only
permissions:

```bash
drag setup
```

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
setup ignores it in favor of Jira's verified account ID. Use `--config <PATH>`
to select another config file. Tokens are never included in JSON output or
debug logs.

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
