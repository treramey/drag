# Architecture

## Workspace

```text
crates/
├── drag/       # I/O-independent parsing, models, and schedules
└── drag-cli/   # Clap, config persistence, HTTP, rendering, process errors
```

The core crate has no terminal, filesystem, or HTTP dependencies. The CLI owns
all side effects and translates typed core failures into stable error/exit
codes.

CLI features use flat sibling modules. The feature module owns its workflow
and external boundaries; an optional `<feature>_tui.rs` sibling owns terminal
state, events, and rendering. Directories are reserved for features with
several peer submodules rather than a single implementation file.

## Command flow

1. Clap parses arguments without terminating the process.
2. `auto` output resolves from stdout's TTY state.
3. Config is loaded from `--config`, `DRAG_CONFIG`, or `~/.drag`.
4. Environment credentials override stored credentials.
5. Core functions validate dates, times, and durations.
6. The API adapter calls Atlassian API v3 and Tempo API v4.
7. Results become human text or a stable JSON envelope.

## Modules

- `drag::time`: duration/interval syntax, date selectors, DST behavior.
- `drag::pagination`: deterministic bounded and exhaustive traversal plans.
- `drag::schedule`: month/day required and logged totals.
- `drag_cli::alias`: alias persistence and presentation.
- `drag_cli::config`: legacy-compatible maps and atomic secret storage.
- `drag_cli::api`: authenticated Jira/Tempo requests, pagination, and response
  decoding.
- `drag_cli::transport`: shared HTTP client policy and bounded retries for
  idempotent reads.
- `drag_cli::doctor`: local diagnostics and optional remote connection checks.
- `drag_cli::error`: typed process and remote-service failures.
- `drag_cli::output`: JSON envelopes, stream selection, and terminal-safe text.
- `drag_cli::schema`: the versioned machine contract, derived from Clap command
  metadata and schemars schemas for shared serde input and result models.
- `drag_cli::setup`: setup state and connection verification.
- Unattended setup dry-runs use the same validated environment credentials and
  verifier boundary as execution, but emit a secret-free plan and never call
  configuration persistence; remote verification requires explicit opt-in.
- `drag_cli::setup_tui`: Ratatui rendering, Crossterm events, and the
  stderr terminal lifecycle for interactive setup.
- `drag_cli::app`: dependency composition and thin use-case routing.

## Safety invariants

- Secrets never appear in result models or debug output.
- Stored setup tokens remain in the workflow boundary and are represented in
  the TUI only as retainable credentials, never as editable field values.
- Config parse errors are never converted into an empty config.
- Config writes use mode `0600` on Unix and a temporary file before replace.
- Authenticated pagination stays on `https://api.tempo.io`.
- List retrieval defaults to 100 records and one page, exposes opaque
  continuations, and requires explicit all-pages traversal; even all-pages
  stops at 100 pages.
- URL path identifiers reject separators, query fragments, percent escapes,
  and control characters.
- Human terminal output strips control, bidirectional override, and zero-width
  characters from remote text; JSON preserves source data.
- Only idempotent reads retry transient transport failures and retryable HTTP
  statuses; mutations are attempted once.
- Mutating worklog and alias operations support `--dry-run`; structured and
  positional worklog deletion share one ordered batch plan, while alias preview
  and execution consume the same normalized config-change plan.

## Adding behavior

Put deterministic calculations and state transitions in `drag`. Keep
filesystem, process, prompt, and HTTP behavior in `drag-cli`. Update
`drag schema`, tests, README examples, and `CHANGELOG.md` when a public
contract changes.
