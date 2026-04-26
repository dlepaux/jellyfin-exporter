<!--
Thanks for the PR. A few quick checks help me get this merged faster:

- Conventional Commits — `feat:`, `fix:`, `docs:`, `refactor:`, `test:`,
  `chore:`, `ci:`, `perf:`. Scopes welcome (`fix(client): ...`).
- Quality gate — `cargo fmt`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test --locked`. CI will run these on push, but local-green
  saves a round trip.
- See [contributing.md](../contributing.md) for the longer version.
-->

## What changes

<!-- One or two sentences. What does this PR do? -->

## Why

<!-- The motivation. What problem does it solve, what use case does it enable? -->

## How tested

<!-- New tests added? Manual verification steps? Output worth pasting? -->

## Breaking change?

<!-- If yes, briefly describe the migration. semantic-release uses the
     conventional-commit footer (`BREAKING CHANGE: ...`) to bump the
     major version, so include it in the commit body if applicable. -->

- [ ] No
- [ ] Yes (describe migration above)

## Checklist

- [ ] Tests added or updated
- [ ] README / metric docs updated if behaviour or surface changed
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --locked` passes
