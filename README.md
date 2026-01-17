# roleman

Roleman is a Rust CLI that uses AWS SSO to select an account/role and set shell environment variables so the AWS CLI works without writing credentials to disk.

## Quick Start

Build and run:

```sh
cargo run -- --sso-start-url https://docker.awsapps.com/start --sso-region us-east-1
```

Install from source:

```sh
cargo install --path .
```

Release tooling:

```sh
cargo install cargo-release
cargo install cargo-dist
cargo release patch
```

The `cargo release` command bumps the version, tags `vX.Y.Z`, and prepares the release for CI.

Enable the zsh hook so `roleman` updates the current shell:

```sh
eval "$(roleman hook zsh)"
```

Then just run:

```sh
roleman --sso-start-url https://docker.awsapps.com/start --sso-region us-east-1
```

To unset all roleman AWS env vars:

```sh
roleman unset
```

To open the selected account/role in the AWS access portal:

```sh
roleman open
```

## Usage

```
roleman [--sso-start-url <url>] [--sso-region <region>] [--account <name>] [--no-cache] [--show-all] [--refresh-seconds <n>] [--env-file <path>] [--print] [--config <path>]
roleman set|s [--account <name>]
roleman open|o [--account <name>]
roleman hook zsh
roleman unset|u
```

## Config

Config lives at `~/.config/roleman/config.toml` and uses TOML.

```toml
default_identity = "work"
refresh_seconds = 300

[[identities]]
name = "work"
start_url = "https://docker.awsapps.com/start"
sso_region = "us-east-1"

ignore_roles = ["ReadOnly"]

# Per-AWS-account rules
accounts = [
  { account_id = "123456789012", alias = "Platform", precedence = 10 },
  { account_id = "999999999999", ignored = true },
  { account_id = "123456789012", ignored_roles = ["Admin"] }
]
```

Notes:
- If multiple accounts are configured and no default is set, Roleman prompts to choose one.
- Use `--account <name>` to select a non-default identity.
- Use `--show-all` to ignore all filters temporarily.

## Shell Hook (zsh)

Install the hook (prints a snippet that updates `_ROLEMAN_HOOK_ENV`):

```sh
eval "$(roleman hook zsh)"
```

Paste it into `~/.zshrc`, then reload your shell.

## Releases

Roleman uses `cargo-dist` to build release artifacts on tag pushes. To create a release:

```sh
cargo release patch
git push --follow-tags
```

The GitHub Action uploads builds and generates the release artifacts automatically.

## Homebrew Tap

The release workflow can publish a Homebrew formula. Configure the tap in `Cargo.toml` under `[workspace.metadata.dist]` (default: `fiam/homebrew-roleman`) and create the repository if it doesn't exist.

Install with:

```sh
brew install fiam/roleman/roleman
```

After each release, update the tap formula to point at the new GitHub release artifact and checksum.

After installing via Homebrew, enable the shell hook:

```sh
eval "$(roleman hook zsh)"
```

## Troubleshooting

Enable trace logging to a file:

```sh
ROLEMAN_LOG_FILE=/tmp/roleman.log RUST_LOG=roleman=trace roleman --sso-start-url https://docker.awsapps.com/start --sso-region us-east-1
```

Check the log for selection and env file write events.
