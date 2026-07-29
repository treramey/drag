# @treramey/drag

## 0.10.0

### Minor Changes

- 67c7307: Offer privacy-aware automatic tracking onboarding after interactive Drag setup
  and through `drag tracking setup`, with independently authorized sources,
  scheduling, hooks, and automatic submission.
- b527564: Make automatic tracking a first-class Drag resource with resumable consolidated
  draft runs, setup, status, review, source, schedule, pause, resume, and safe
  uninstall workflows backed by the separately packaged tracking process.
- 6deafb3: Enable guarded review-mode and explicitly authorized automatic tracking submissions.
- eb76764: Report tracking schedule health and validate configuration before resuming automatic tracking.
- d33db7d: Report immutable tracking proposal reviews with redacted evidence, policy conflicts, approval availability, stale-set invalidation, and preserved runtime submission gates.
- f37881b: Finish the intent-level automatic tracking command transition, harden guarded submissions against stale approvals, runtime gate races, and uncertain partial progress, safely install Claude hooks, and document the compatibility removal point.
- b527564: Add validated, redacted, and bounded tracking evidence source management, with
  date-scoped collection, failure-gated complete runs, and proposal-stage wiring.

### Patch Changes

- 499eb38: Default Claude hook installation and removal to `~/.claude/settings.json` when `--settings` is omitted.
- ea48c53: Propagate custom tracking data directories into installed Claude Code capture hooks.
- b527564: Make legacy companion state migration resumable, preserve recovery state, and report explicit environment conflicts and recovery guidance.

## 0.9.0

### Minor Changes

- 7aa0e70: Add the end-of-day `drag-companion` and `drag resolve` workflows with safe capture, evidence journaling, deterministic bundles, local Git/ICS/Claude collection, schema-constrained proposal fixtures, Drag CLI reconciliation, guarded policy/audit/preview flows, staged rollout-gated execution, durable run coordination, replay reports, scheduler lifecycle management, retention, purge, operator recovery reporting, and distribution of `drag-companion` alongside `drag` in release archives, npm, and Homebrew.

## 0.8.1

### Patch Changes

- 963cb0a: Include existing Tempo work attributes directly in structured list results.

## 0.8.0

### Minor Changes

- Polish the interactive list dashboard hierarchy and clarify schedule progress.

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
