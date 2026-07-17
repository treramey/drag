# Agent usage context

- Run `drag --output json schema` before constructing commands dynamically.
- Always pass `--output json` in automation; do not rely on TTY detection.
- Prefer `drag log --json -` with stdin for structured input.
- Use `--dry-run` before `log` or `delete` mutations.
- Read successful data from stdout and structured failures from stderr.
- Exit `2` means the request must be corrected; exit `1` means config,
  network, server, or local I/O failed.
- Use `--verbose` only when descriptions and issue URLs are needed.
- Keep `list` bounded: start with its 100-record/one-page defaults, then pass
  `pagination.next` unchanged to `--continue-from`. Use `--all-pages` only for
  intentional exhaustive reads; it still stops at the 100-page ceiling.
- Never request, print, or copy values from the stored token fields.
