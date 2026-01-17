# Repository Guidelines

## Project Structure & Module Organization
- `src/lib.rs` hosts the library API and core modules (SSO cache, AWS SDK calls, selection UI).
- `src/main.rs` is a thin CLI wrapper around the library.
- `Cargo.toml` defines the crate metadata and dependencies.
- Keep feature modules under `src/` (e.g., `src/sso.rs`, `src/cache.rs`, `src/tui.rs`).
- Add integration tests under `tests/` once CLI behavior is stable.

## Build, Test, and Development Commands
- `cargo run` — build and run the binary locally.
- `cargo build` — compile the project in debug mode.
- `cargo build --release` — produce an optimized release binary.
- `cargo test` — run all unit and integration tests.
- `cargo run -- --no-cache` — force SSO sign-in instead of using cached tokens.
- `cargo run -- --show-all` — ignore any account/role filters configured for the selected account.
- `cargo run -- hook zsh` — print the zsh hook snippet for env updates.
- `ROLEMAN_LOG_FILE=/tmp/roleman.log RUST_LOG=roleman=trace cargo run -- ...` — log trace output to a file to avoid TUI clearing logs.
- `cargo run -- --print` — print env exports to stdout (default is hook-only).
- `cargo run -- open` — open the selected account/role in the AWS access portal.
- `cargo run -- unset` — print an `unset` line to clear roleman environment variables.

## Coding Style & Naming Conventions
- Use standard Rust formatting via `rustfmt` (e.g., `cargo fmt`).
- Indentation is 4 spaces (Rust defaults).
- Prefer snake_case for functions/modules and CamelCase for types.
- Keep modules small and focused; split new features into `src/<feature>.rs` plus `mod` declarations.
- Prefer small pure functions to make AWS SDK calls and cache parsing easy to test.
- Use async/await with Tokio for AWS SDK calls; avoid blocking in async code paths.

## Testing Guidelines
- Use Rust’s built-in test framework (`#[test]`) for unit tests in `src/`.
- For integration tests, add files under `tests/` (e.g., `tests/cli.rs`).
- Cover AWS SSO cache parsing, env var export formatting, and CLI delegation paths.
- Name tests descriptively, e.g., `parses_sso_cache` or `rejects_empty_input`.

## Commit & Pull Request Guidelines
- No explicit commit message convention is established in this repo; use short, imperative summaries (e.g., "Add config parser").
- PRs should include a clear description of changes, how to test, and any relevant screenshots or logs if behavior changes.

## Configuration & UX Notes
- The tool accepts an SSO start URL via CLI args or via `~/.config/roleman/config.toml`.
- Config keys: `identities` (list of `{ name, start_url, sso_region, accounts, ignore_roles }`), `default_identity`, and `refresh_seconds`.
- Each `accounts` entry supports `{ account_id, alias, ignored, ignored_roles, precedence }` where higher precedence appears first.
- Use `--account <name>` to select a non-default identity when multiple are configured.
- The selector TUI should be fzf-style (non-fullscreen) and not take over the terminal when possible.
- Avoid writing to `~/.aws/config` or `~/.aws/credentials`; rely on env exports and device-authorization login when no cache is present.
- When `AWS_PROFILE` is set, write a minimal profile section to a Roleman-managed config file under `$XDG_STATE_HOME/roleman/aws-config` (or `~/.local/state/roleman/aws-config`) and export `AWS_CONFIG_FILE` to point at it.
- Reuse AWS SSO cache from `~/.aws/sso/cache`, but write refreshed tokens to the Roleman cache under `~/.cache/roleman` (or `$XDG_CACHE_HOME/roleman`).
- Cache account/role listings for 24 hours in the Roleman cache to avoid unnecessary API calls (skip with `--no-cache`).
- For long-running sessions, support periodic refresh of account/role lists via `refresh_seconds`.
- For shell integration, `roleman hook zsh` prints a zsh hook that sources a per-TTY env file if it exists and deletes it after sourcing. The hook uses `_ROLEMAN_HOOK_ENV` internally.
