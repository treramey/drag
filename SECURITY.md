# Security policy

Only the latest release receives security fixes while the rewrite is pre-1.0.

Report vulnerabilities with GitHub private vulnerability reporting under
**Security → Report a vulnerability**. Do not open a public issue containing
tokens, account data, Jira hostnames, worklogs, or exploit details.

Include the affected version, platform, reproduction, impact, and suggested
fix if known. Maintainers should acknowledge reports within seven days and
coordinate disclosure after a patch is available.

## Setup credential handling

Interactive `drag setup` masks typed and pasted Jira and Tempo tokens. Stored
tokens can be retained without loading them into editable terminal fields, and
the final review contains only non-secret identity and connection status.
Browser assistance always leaves the token URL visible; `--no-open` prevents a
browser launch without changing token handling.

Setup verifies both services with read-only requests and writes configuration
only after the explicit Save action. Escape or Ctrl-C cancellation, validation
errors, failed verification, and terminal errors leave the existing file
unchanged. `drag setup --from-env` is the non-interactive path and does not open
a browser or initialize Ratatui. Avoid exposing its four credential environment
variables through shell history, process diagnostics, CI logs, or debug output.
Use `drag --output json setup --from-env --dry-run` to validate and inspect a
secret-free unattended plan without network access or writes. Add `--verify`
only when read-only remote checks are intended. Setup does not accept Jira or
Tempo tokens in arguments or JSON. Missing, empty, non-Unicode, control-bearing,
or unsafe Jira-site environment values are rejected before any write.

On Unix, saved configuration is restricted to the current user. Tokens are
excluded from human and JSON results, errors, diagnostics, and TUI review
screens. Treat the config file and environment variables as secrets despite
those output protections.

## Remote-content rendering

Treat worklog descriptions, issue identifiers, Jira URLs, and API error details
as untrusted data. Drag preserves those values in structured JSON, where the
JSON serializer escapes them as string contents. In human and TUI output, Drag
renders control characters, line breaks, tabs, bidirectional formatting, and
zero-width characters as visible escapes so a remote value cannot add a row,
warning, diagnostic, or terminal control sequence.

Generated labels such as `error:`, `warning:`, and table column headings remain
outside escaped remote values. This boundary is syntactic: Drag does not claim
to detect the meaning of prose or prevent semantic prompt injection. Consumers
must continue treating every remote string as data rather than instructions.
