# Changelog

This project follows [Semantic Versioning](https://semver.org/) and
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Guided Jira and Tempo setup with read-only credential verification,
  automatic Atlassian account-ID discovery, safe credential reuse, and one
  transactional save.
- Browser-assisted Atlassian and Tempo token generation, with visible fallback
  links and a `--no-open` option.
- Opt-in `doctor --remote` checks for read-only Jira and Tempo connectivity,
  with stable per-service human and JSON results.
- Verified headless setup using a reduced set of four environment variables.
- Worklog, alias, schedule, and tracker behavior.
- Tempo API v4 and Atlassian API v3 clients using Rustls.
- Compatibility with the original map and tracker format.
- Human/JSON output, structured errors, schema discovery, and diagnostics.
- Raw JSON input, environment-based headless setup, and mutation dry runs.
- Cross-platform CI, dependency policy, audits, and release artifacts.

### Security

- Restrict authenticated Tempo pagination to the Tempo API origin.
- Validate URL path identifiers and redact all credential output.
- Report malformed config instead of silently replacing it with empty state.
