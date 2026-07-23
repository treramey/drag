## Agent skills

### Changesets

User-visible pull requests MUST include `.changeset/<descriptive-name>.md` for
`@treramey/drag`: use `patch` for fixes/chores, `minor` for features, and
`major` for breaking changes. Run `pnpm changeset` to create it. Documentation-
only and internal CI changes do not require a changeset.

### Issue tracker

Issues are tracked in GitHub Issues using the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

This repository uses the five default triage labels. See `docs/agents/triage-labels.md`.

### Domain docs

This repository uses a single-context domain-doc layout. See `docs/agents/domain.md`.
