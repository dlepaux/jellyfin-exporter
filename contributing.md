# Contributing

Thanks for your interest in contributing!

## Getting Started

1. Fork the repository
2. Clone your fork
3. Create a feature branch: `git checkout -b feat/my-feature`
4. Make your changes

## Development

```bash
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

All four must pass before submitting a PR.

## Commit Messages

This project uses [Conventional Commits](https://www.conventionalcommits.org/):

- `feat:` — new feature
- `fix:` — bug fix
- `docs:` — documentation only
- `refactor:` — code change that neither fixes a bug nor adds a feature
- `test:` — adding or updating tests
- `chore:` — maintenance

## Pull Requests

- Keep PRs focused — one feature or fix per PR
- Include a clear description of what changed and why
- Ensure CI passes before requesting review

## Reporting Issues

File issues via the [issue forms](https://github.com/dlepaux/jellyfin-exporter/issues/new/choose) — they capture the fields needed for triage (exporter version, Jellyfin version, deployment shape, redacted logs). Blank issues are disabled; pick the closest template and fill in what you can.

For security vulnerabilities, **do not** open a public issue. See [security.md](security.md) — GitHub Security Advisories or email get you a private channel.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](license.md).
