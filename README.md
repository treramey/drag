# Tempomat for Rust

A fast, scriptable CLI for logging and tracking time in
[Tempo Cloud](https://tempo.io). This is a Rust rewrite of
[`szymonkozak/tempomat`](https://github.com/szymonkozak/tempomat), preserving
its practical command shortcuts and `~/.tempomat` data while adding structured
output, dry runs, safer HTTP behavior, and cross-platform binaries.

> The rewrite is currently pre-1.0. Exercise `--dry-run` and verify behavior in
> a non-production Tempo account before replacing an existing installation.

## Features

- Log work using durations (`1h15m`) or intervals (`11-12:30`).
- List daily worklogs with monthly required/logged totals.
- Delete one or several worklogs.
- Store aliases such as `lunch => ABC-123`.
- Start, pause, resume, inspect, and stop persistent local trackers.
- Read the original TypeScript CLI's `~/.tempomat` config and tracker format.
- Return readable terminal output or consistent JSON for scripts and agents.
- Preview mutations with `--dry-run`.

## Install from source

```bash
cargo install --path crates/tempomat-cli
tempo --version
```

Version tags also produce checksummed Linux, macOS, and Windows binaries in
GitHub Releases.

## Configuration

Interactive setup stores credentials in `~/.tempomat` with user-only file
permissions:

```bash
tempo setup
```

For headless use, set all variables and persist them with `tempo setup
--from-env`, or leave them in the environment to override stored values:

```bash
export TEMPO_TOKEN=...
export TEMPO_ACCOUNT_ID=...
export ATLASSIAN_EMAIL=you@example.com
export ATLASSIAN_TOKEN=...
export ATLASSIAN_HOST=yourcompany.atlassian.net
tempo setup --from-env
```

Use `TEMPOMAT_CONFIG` or `--config <PATH>` to select another config file.
Tokens are never included in JSON output or debug logs.

## Usage

```bash
# Worklogs
tempo log ABC-123 1h15m
tempo l ABC-123 11-12:30 yesterday -d "review"
tempo log lunch 30m --start 12:00 --dry-run
tempo list
tempo ls 2026-07-14 --verbose
tempo delete 123456 123457

# Aliases (both modern and original colon forms work)
tempo alias set lunch ABC-123
tempo alias list
tempo alias:set lunch ABC-123

# Trackers
tempo tracker start ABC-123 -d "implementation"
tempo pause ABC-123
tempo resume ABC-123
tempo tracker list
tempo stop ABC-123 --dry-run
tempo stop ABC-123
```

Accepted date selectors are `YYYY-MM-DD`, `y`, `yesterday`, `t±N`, and
`today±N`. Intervals that end before their start cross midnight.

### JSON and raw input

Output defaults to human text in a terminal and JSON when redirected. Pin the
contract explicitly in automation:

```bash
tempo --output json list | jq
tempo --output json schema
printf '%s' '{"issueKeyOrAlias":"ABC-1","durationOrInterval":"30m"}' \
  | tempo --output json log --json - --dry-run
```

Successful JSON uses `{"ok":true,"data":...}`. Errors go to stderr as
`{"ok":false,"error":{"code":"...","message":"..."}}`.

| Exit | Meaning |
|---:|---|
| `0` | Success |
| `1` | Config, network, server, or I/O failure |
| `2` | Invalid command, input, date, or duration |

## Compatibility with Tempomat 2.x

The binary remains `tempo`; shortcuts `l`, `ls`, `d`, `start`, `pause`,
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

## Attribution and license

This project is based on the original Tempomat by Szymon Kozak. Both the
original project and this rewrite are MIT licensed. See [LICENSE](LICENSE).

