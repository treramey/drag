# @treramey/drag

## 0.7.2

### Patch Changes

- Authenticate Git before publishing the generated Homebrew formula.

## 0.7.1

### Patch Changes

- Support Tempo work attributes in `drag log` through JSON input and repeatable `--attr KEY=VALUE` flags, with actionable hints for required attributes.

## 0.7.0

### Minor Changes

- 3248fe0: Add portable coding-session worklog recipes and conservative Tempo operation effect labels.

## 0.6.0

### Minor Changes

- 7c0f785: Generate installable AI agent skills from Drag's command contract and Tempo's live OpenAPI catalog.
- a92fd56: Remove issue-key alias commands, persisted aliases, alias resolution, and alias-aware list labels; log JSON now uses `issueKey`.
- a92fd56: Remove the `completions` command and its `autocomplete` alias.
- d62c23a: Print schema JSON directly in human terminals and inspect dotted Tempo component schemas as well as operations.

### Patch Changes

- a92fd56: Show generated Tempo command help successfully when no resource is supplied.

## 0.5.0

### Minor Changes

- d55d365: Generate Tempo API v4 commands from the official OpenAPI document with schema inspection, validated generic JSON bodies and parameters, authenticated execution, caching, and dry-run previews.

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
