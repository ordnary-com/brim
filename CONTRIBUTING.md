# Contributing to Brim

Thank you for helping build Brim. This project is in an early phase, so design discussions matter as much as code.

## Before you start

- For language changes (syntax, semantics, typing rules), open an issue first. Changes to the language must update [`docs/language-spec.md`](docs/language-spec.md) in the same pull request.
- For compiler changes, read [`docs/architecture.md`](docs/architecture.md) and [`docs/development.md`](docs/development.md).

## Pull request checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] New behavior has tests: unit tests in the affected crate, plus fixtures in `tests/` for user-visible behavior
- [ ] New diagnostic codes are documented in [`docs/error-handling.md`](docs/error-handling.md)
- [ ] Documentation describes what actually works: unimplemented features are never described as available
- [ ] `CHANGELOG.md` has an entry under "Unreleased"

## Principles

- Keep the workspace buildable and the tests green on every commit.
- The compiler must never panic on user input. Report a diagnostic instead.
- No `unsafe` code without a documented justification (it is forbidden by default).
- Keep dependencies minimal and pinned. The frontend must not depend on LLVM.
- Prefer simple, explicit code over cleverness.

## License

By contributing, you agree that your contributions are licensed under the Apache License, Version 2.0, as described in [LICENSE](LICENSE).
