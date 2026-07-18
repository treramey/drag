# Agent usage context

- Run `drag --output json schema` before constructing commands dynamically.
- Always pass `--output json` in automation; use explicit `--output ndjson` only
  for incremental `list` processing. Do not rely on TTY detection.
- Prefer `drag log --json -` with stdin for structured input.
- Use `--dry-run` before `log` or `delete` mutations.
- Read successful data from stdout and structured failures from stderr.
- Exit `2` means the request must be corrected; exit `1` means config,
  network, server, or local I/O failed.
- Use `--verbose` only when descriptions and issue URLs are needed.
- Use `list --fields` in automation and request only needed result leaves. For
  traversal, include `pagination.next` and `pagination.selectedDate`; omit the
  mask only when the complete list report is required.
- In NDJSON list output, parse each line independently by `kind`: zero or more
  `worklog` events are followed by `summary` and terminal `pagination` events.
  Worklogs are emitted page-by-page. A network or enrichment failure has no
  summary or terminal event; retain prior lines and read the structured failure
  from stderr.
- Keep `list` bounded: start with its 100-record/one-page defaults, then pass
  `pagination.next` unchanged to `--continue-from` and reuse
  `pagination.selectedDate` as `DATE`. Omit pagination flags to restore the
  token's bounds, or repeat them exactly. Use `--all-pages` only for
  intentional exhaustive reads; it still stops at the 100-page ceiling.
- Never request, print, or copy values from the stored token fields.
