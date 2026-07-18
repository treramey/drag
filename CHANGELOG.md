# Changelog

This project follows [Semantic Versioning](https://semver.org/) and
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Interactive Ratatui list reports for fully attached non-verbose human
  terminals, with qualified partial totals, populated and empty worklog views,
  and clean quit keys.
- Explicit NDJSON list streaming with discriminated worklog, schedule summary,
  and terminal pagination events; page-wise bounded retrieval; field-aware Jira
  enrichment; and structured mid-stream failures that preserve prior lines.
- Deterministic `list --fields` projection for worklog, schedule, and pagination
  results, with pre-network validation and unchanged human rendering.
- Bounded `list` retrieval with finite record/page defaults, deterministic
  continuation metadata bound to the selected date and effective pagination
  plan, and explicit all-pages traversal under a hard 100-page ceiling.
- Secret-free unattended setup dry-run plans with optional read-only Jira and
  Tempo verification and no configuration writes.
- Structured ordered worklog deletion through inline or stdin JSON, sharing the
  existing batch and dry-run behavior with positional IDs.
- Ratatui onboarding for interactive setup, with masked token input,
  asynchronous connection progress, backward navigation, safe stored-token
  reuse and replacement, an explicit review-and-save step, an interruptible
  brand reveal, animated focused-input borders, playful pending, connection,
  and review feedback with a reduced-motion mode, responsive resize handling,
  and actionable guidance for undersized terminals.
- Guided Jira and Tempo setup with read-only credential verification,
  automatic Atlassian account-ID discovery, safe credential reuse, and one
  transactional save.
- Browser-assisted Atlassian and Tempo token generation, with visible fallback
  links and a `--no-open` option.
- Opt-in `doctor --remote` checks for read-only Jira and Tempo connectivity,
  with stable per-service human and JSON results.
- Verified headless setup using a reduced set of four environment variables.
- Worklog, alias, and schedule behavior.
- Tempo API v4 and Atlassian API v3 clients using Rustls.
- Compatibility with the original map format.
- Human/JSON output, structured errors, schema discovery, and diagnostics.
- A complete versioned machine-readable CLI contract derived from Clap and
  shared serde models, covering commands, compatibility forms, inputs, result
  schemas, structured errors, side effects, network access, and dry runs.
- Raw JSON input, environment-based headless setup, and mutation dry runs.
- Typed inline and stdin JSON for alias set/delete, with shared normalized
  create, replace, delete, and unchanged plans and config-safe dry runs.
- Cross-platform CI, dependency policy, audits, and release artifacts.

### Fixed

- Build human and structured list output from one shared immutable report model
  without changing existing output contracts.
- Centralize CLI errors, output, schema, diagnostics, aliases, and HTTP policy
  behind owned modules; reuse pooled HTTP connections and retry only
  idempotent reads after bounded transient failures.
- Lock down the mutating `log`/`l` contract: duration, interval, date, start,
  description, and remaining-estimate inputs; DST-aware overnight intervals;
  network-free dry runs; ordered Jira-to-Tempo creation; structured failures;
  redacted diagnostics; and complete help and schema metadata.
- Lock down the read-only `list`/`ls` contract: local-time date defaults and
  relative selectors, inclusive calendar-month totals, structured failures,
  stable JSON output, and safe multi-page Tempo worklog retrieval.

### Removed

- Local tracker commands, compatibility aliases, persisted tracker state, and
  tracker upload behavior.

### Security

- Keep untrusted Jira and Tempo values within their human-rendering fields by
  visibly escaping line breaks, controls, bidirectional formatting, and
  zero-width characters while preserving source strings in JSON.
- Validate caller-provided and server-provided Tempo continuation URLs before
  authenticated requests, rejecting malformed, credential-bearing,
  cross-origin, and selected-month-mismatched URLs without echoing them or
  rewriting opaque continuation queries.
- Sanitize remote text before human terminal rendering to remove control,
  bidirectional override, and zero-width characters without changing JSON.
- Restore terminal raw mode, alternate screen, cursor visibility, and bracketed
  paste after onboarding success, cancellation, errors, and panics.
- Keep Ratatui rendering on standard error so successful JSON output remains
  parseable and free of terminal control sequences.
- Restrict authenticated Tempo pagination to the Tempo API origin.
- Validate URL path identifiers and redact all credential output.
- Report malformed config instead of silently replacing it with empty state.
