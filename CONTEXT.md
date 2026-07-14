# Agent usage context

- Run `tempo --output json schema` before constructing commands dynamically.
- Always pass `--output json` in automation; do not rely on TTY detection.
- Prefer `tempo log --json -` with stdin for structured input.
- Use `--dry-run` before `log`, `delete`, or `tracker stop` mutations.
- Read successful data from stdout and structured failures from stderr.
- Exit `2` means the request must be corrected; exit `1` means config,
  network, server, or local I/O failed.
- Use `--verbose` only when descriptions and issue URLs are needed.
- Never request, print, or copy values from the stored token fields.

