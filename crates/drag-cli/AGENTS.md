## Side-effect tests

- Exercise setup/doctor networking through `App`'s `ConnectionVerifier`; fixed production origins make localhost stubs misleading, and tests must never contact live services.
- Keep hidden-input coverage on the ignored in-process PTY helper so prompts and verifier responses remain deterministic.
