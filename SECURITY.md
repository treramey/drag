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

On Unix, saved configuration is restricted to the current user. Tokens are
excluded from human and JSON results, errors, diagnostics, and TUI review
screens. Treat the config file and environment variables as secrets despite
those output protections.
