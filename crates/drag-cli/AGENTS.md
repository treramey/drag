## Side-effect tests

- Exercise setup/doctor networking through `App`'s `ConnectionVerifier`; fixed production origins make localhost stubs misleading, and tests must never contact live services.
- Clear all five connection environment variables in CLI subprocess helpers before setting scenario-specific values.
- Keep hidden-input coverage on the ignored in-process PTY helper so prompts and verifier responses remain deterministic.

## Feature layout

- Keep feature workflow in `<feature>.rs` and terminal state, events, and rendering in a flat `<feature>_tui.rs` sibling; see `docs/architecture.md`.
