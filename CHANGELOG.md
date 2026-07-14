# Changelog

This project follows [Semantic Versioning](https://semver.org/) and
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Rust rewrite of Tempomat's worklog, alias, schedule, and tracker behavior.
- Tempo API v4 and Atlassian API v3 clients using Rustls.
- Compatibility with the original `~/.tempomat` map and tracker format.
- Human/JSON output, structured errors, schema discovery, and diagnostics.
- Raw JSON input, environment-based headless setup, and mutation dry runs.
- Cross-platform CI, dependency policy, audits, and release artifacts.

### Security

- Restrict authenticated Tempo pagination to the Tempo API origin.
- Validate URL path identifiers and redact all credential output.
- Report malformed config instead of silently replacing it with empty state.

