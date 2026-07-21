# Contributing

Thank you for improving Drag. Please discuss substantial
behavior or compatibility changes in an issue before implementation.

## Local checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
```

If installed, also run `cargo deny check`. Never run integration experiments
against another person's Tempo account.

Changes to date/time syntax should add table-driven tests in
`crates/drag/src/time.rs`. Config changes must retain fixtures for the
TypeScript `Map` format. API changes should preserve redaction and URL
validation. Public changes require updates to
`drag schema`, README examples, and `CHANGELOG.md`.

User-visible changes also require a changeset. Run `pnpm install` once, then
`pnpm changeset`, select `@treramey/drag`, and commit the generated file.
Merging the resulting release-version PR prepares the Cargo, npm, and changelog
versions; pushing its `vX.Y.Z` tag publishes the distribution artifacts.
The Changesets workflow uses the repository's `RELEASE_TOKEN` bot credential so
that a pushed release tag can trigger the native distribution workflow.

Do not commit tokens, account IDs, private Jira hostnames, API response dumps,
or real worklog descriptions. See [SECURITY.md](SECURITY.md) for private
vulnerability reporting.
