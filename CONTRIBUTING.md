# Contributing

Thank you for improving the Tempomat Rust rewrite. Please discuss substantial
behavior or compatibility changes in an issue before implementation.

## Local checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

If installed, also run `cargo deny check`. Never run integration experiments
against another person's Tempo account.

Changes to date/time syntax should add table-driven tests in
`crates/tempomat/src/time.rs`. Config changes must retain fixtures for the
TypeScript `Map` format. API changes should preserve redaction, URL validation,
and tracker partial-failure recovery. Public changes require updates to
`tempo schema`, README examples, and `CHANGELOG.md`.

Do not commit tokens, account IDs, private Jira hostnames, API response dumps,
or real worklog descriptions. See [SECURITY.md](SECURITY.md) for private
vulnerability reporting.

