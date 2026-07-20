# @treramey/drag

## 0.4.0

### Minor Changes

- 26349ac: Open completed non-verbose human list reports in Ratatui when all terminal streams are attached, while qualifying partial segments and preserving verbose, redirected, and structured output.
- d9e0d0d: Add focused-row navigation, scrolling, responsive columns, and verbose worklog details to the interactive list report.
- e275798: Open the focused interactive list worklog's resolved Jira URL with `o` and keep the report usable after browser success or failure.

### Patch Changes

- 892ece3: Build list presentations from one shared immutable report model while preserving existing output.

## 0.3.0

### Minor Changes

- f535432: Bound list retrieval by default and add deterministic continuation and explicit all-pages controls.
- 4602355: Add validated field selection to structured list output.
- b169fdd: Add secret-free unattended setup dry-run plans with optional read-only verification.
- a7b4762: Stream bounded list results page-by-page as discriminated NDJSON worklog, summary, and pagination events.
- b018602: Accept ordered worklog deletion batches as inline or stdin JSON while preserving positional and dry-run behavior.

### Patch Changes

- a32f2f4: Keep untrusted Jira and Tempo content inside clearly delimited terminal fields without changing structured JSON values.

## 0.2.0

### Minor Changes

- a1fbbc8: Publish a complete versioned machine-readable contract for every CLI command, input, result, error, side effect, network operation, and dry-run mode.
- c9fbc15: Add npm, Nix, Homebrew, checksummed native binary, and provenance-aware release pipelines.
- ac9cd58: Add typed inline and stdin JSON for alias set/delete plus normalized, config-safe dry-run plans for create, replace, delete, and unchanged operations.

### Patch Changes

- 12244b7: Harden terminal rendering and transient read retries while separating CLI contracts into owned Rust modules.
