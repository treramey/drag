# Architecture

## Workspace

```text
crates/
├── drag/       # I/O-independent parsing, models, schedules, tracker state
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
5. Core functions validate dates, times, durations, and tracker transitions.
6. The API adapter calls Atlassian API v3 and Tempo API v4.
7. Results become human text or a stable JSON envelope.

## Modules

- `drag::time`: duration/interval syntax, date selectors, DST behavior.
- `drag::schedule`: month/day required and logged totals.
- `drag::tracker`: persistent timer state machine.
- `drag_cli::config`: legacy-compatible maps and atomic secret storage.
- `drag_cli::api`: authentication, pagination, endpoint validation.
- `drag_cli::setup`: setup state and connection verification.
- `drag_cli::setup_tui`: Ratatui rendering, Crossterm events, and the
  stderr terminal lifecycle for interactive setup.
- `drag_cli::app`: use-case orchestration and partial tracker upload safety.

## Safety invariants

- Secrets never appear in result models or debug output.
- Config parse errors are never converted into an empty config.
- Config writes use mode `0600` on Unix and a temporary file before replace.
- Authenticated pagination stays on `https://api.tempo.io`.
- URL path identifiers reject separators, query fragments, percent escapes,
  and control characters.
- Mutating worklog and tracker-stop operations support `--dry-run`.
- Successfully uploaded tracker intervals are removed immediately; failed
  intervals remain locally recoverable.

## Adding behavior

Put deterministic calculations and state transitions in `drag`. Keep
filesystem, process, prompt, and HTTP behavior in `drag-cli`. Update
`drag schema`, tests, README examples, and `CHANGELOG.md` when a public
contract changes.
