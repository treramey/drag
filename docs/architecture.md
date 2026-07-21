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
7. Results become human text, a stable JSON envelope, or explicit NDJSON list
   events written and flushed one line at a time.

## Modules

- `drag::time`: duration/interval syntax, date selectors, DST behavior.
- `drag::field_selection`: validated list masks and deterministic projection of
  complete reports into stable structured shapes.
- `drag::pagination`: deterministic bounded and exhaustive traversal plans.
- `drag::schedule`: month/day required and logged totals.
- `drag_cli::browser`: shared boundary for local default-browser launches.
- `drag_cli::config`: legacy-compatible maps and atomic secret storage.
- `drag_cli::api`: authenticated Jira/Tempo requests, pagination, and response
  decoding.
- `drag_cli::transport`: shared HTTP client policy and bounded retries for
  idempotent reads.
- `drag_cli::update`: best-effort, time-bounded latest-release discovery for
  the interactive list header; failures never affect command success.
- `drag_cli::doctor`: local diagnostics and optional remote connection checks.
- `drag_cli::error`: typed process and remote-service failures.
- `drag_cli::output`: JSON envelopes, stream selection, and terminal-safe text.
- `drag_cli::list`: retrieval and enrichment produce one immutable list report
  containing the selected date, worklogs, schedule and pagination details,
  issue labels, and verbose state. Plain text and structured JSON
  are projections of that completed report.
- `drag_cli::list_tui`: eligibility checks, focused row state, bounded keyboard
  navigation, responsive stderr Ratatui rendering, scrolling, verbose focused
  details, recoverable opening of the focused worklog's resolved Jira URL,
  quit-event handling, and restoration of terminal state for
  interactive list reports. Retrieval completes before this boundary is
  entered.
- `drag_cli::schema`: the versioned machine contract, derived from Clap command
  metadata and schemars schemas for shared serde input and result models.
- `drag_cli::generate_skills`: deterministic local Agent Skill rendering from
  the machine contract and curated portable recipe registry, plus progressively
  disclosed, effect-classified Tempo resource references from the official
  OpenAPI operation catalog.
- `drag_cli::tempo_openapi`: fixed-origin Tempo OpenAPI discovery, bounded YAML
  parsing, 24-hour ETag-aware caching, dotted operation lookup, local
  component-reference resolution, generated read-only command trees, and
  validated path/query request preparation.
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
- OpenAPI discovery stays on `https://apidocs.tempo.io/tempo-openapi.yaml`,
  limits documents to 2 MiB, and atomically replaces its disposable cache.
- Generated Tempo commands accept API v4 operations, build URLs from fixed
  OpenAPI paths, encode path segments, validate declared parameters and JSON
  request bodies, and keep bearer-authenticated requests on the configured
  Tempo origin. Dry-runs stop before credential loading and API access, and the
  shared transport retries GET requests but never mutations.
- List retrieval defaults to 100 records and one page, exposes opaque
  continuations, and requires explicit all-pages traversal; even all-pages
  stops at 100 pages.
- List field masks are validated before configuration or networking. Projection
  is deterministic, occurs before the output envelope is serialized, and does
  not change schedule calculations, pagination state, or human rendering.
- NDJSON list output fetches one validated Tempo page at a time, incrementally
  accumulates schedule totals, and emits field-aware `worklog` events before
  requesting the next page. A network or enrichment failure leaves prior
  stdout lines valid, omits the `summary` and terminal `pagination` events, and
  uses the normal stderr error envelope; an intentional stdout broken pipe is
  success.
- URL path identifiers reject separators, query fragments, percent escapes,
  and control characters.
- Human and TUI renderers visibly escape line breaks, tabs, controls,
  bidirectional formatting, and zero-width characters inside remote data;
  generated labels remain outside those values, while JSON preserves source
  strings through normal serializer escaping.
- Interactive list browser launches consume the already resolved Jira browse
  URL and never perform another Jira or Tempo request. Launch failures remain
  local, redacted status messages and do not terminate the report.
- Only idempotent reads retry transient transport failures and retryable HTTP
  statuses; mutations are attempted once.
- Mutating worklog operations support `--dry-run`; structured and positional
  worklog deletion share one ordered batch plan.

## Adding behavior

Put deterministic calculations and state transitions in `drag`. Keep
filesystem, process, prompt, and HTTP behavior in `drag-cli`. Update
`drag schema`, tests, README examples, and `CHANGELOG.md` when a public
contract changes.
