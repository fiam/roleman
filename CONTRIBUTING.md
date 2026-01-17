# Contributing

Thanks for contributing to Roleman. This guide explains how to develop, test, and cut releases.

## Development

- Prerequisites: Rust stable, `cargo`, and (optionally) `cargo-release`/`cargo-dist`.
- Install dependencies:

```sh
rustup update stable
```

- Build and run locally:

```sh
cargo run -- --sso-start-url https://example.awsapps.com/start --sso-region us-east-1
```

## Testing and Linting

Run the full local suite before opening a PR:

```sh
cargo fmt --all -- --check
cargo check
cargo clippy -- -D warnings
cargo test
```

## Pull Requests

- Keep changes focused; include tests for new behavior.
- Update documentation when CLI or config changes.
- PRs must pass CI (fmt, clippy, tests).

## Releases

Roleman uses `cargo-release` for versioning and `cargo-dist` for release artifacts.

1) Install release tooling:

```sh
cargo install cargo-release
cargo install cargo-dist
```

2) Cut a release (example for patch):

```sh
cargo release patch
```

3) Push the tag:

```sh
git push --follow-tags
```

The GitHub Actions release workflow runs on tag pushes, builds artifacts with `cargo-dist`, and publishes the GitHub Release plus Homebrew formula updates.
