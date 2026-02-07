# roleman

Roleman is a Rust CLI for AWS IAM Identity Center (AWS SSO).
It lets you choose an account and role, then exports temporary AWS env vars into your shell.

## Getting Started

### 1. Install roleman

#### Homebrew (recommended)

```sh
brew tap fiam/roleman
brew install fiam/roleman/roleman
```

#### GitHub Releases

1. Download the archive for your OS/CPU from [Releases](https://github.com/fiam/roleman/releases).
2. Extract it.
3. Move `roleman` into your `PATH`.

Example:

```sh
tar -xzf roleman-<target>.tar.gz
chmod +x roleman
sudo mv roleman /usr/local/bin/roleman
```

#### Build from source

```sh
cargo install --path .
```

### 2. Enable the shell hook (required)

Roleman updates your current shell through a hook file. Supported shells: zsh and bash.

Quick test in the current shell:

```sh
eval "$(roleman hook zsh)"
```

Install permanently into your shell rc file (`~/.zshrc` or `~/.bashrc`):

```sh
roleman install-hook
```

Optional alias:

```sh
roleman install-hook --alias
```

Reload your shell after installing:

```sh
source ~/.zshrc
```

### 3. Configure your SSO identity

Create `~/.config/roleman/config.toml`:

```toml
default_identity = "work"

[[identities]]
name = "work"
start_url = "https://acme.awsapps.com/start"
sso_region = "us-east-1"
```

Now run:

```sh
roleman
```

On first run, Roleman uses device auth if needed, lets you pick an account/role, and exports AWS env vars.

## Daily Usage

Set credentials by picking an account/role:

```sh
roleman
# or
roleman set
```

Open the selected role directly in the AWS access portal:

```sh
roleman open
```

Clear Roleman-managed AWS env vars:

```sh
roleman unset
```

Force a fresh SSO flow and skip cache:

```sh
roleman --no-cache
```

Temporarily ignore configured account/role filters:

```sh
roleman --show-all
```

Use a non-default configured identity:

```sh
roleman --account prod
```

## Configuration

Path: `~/.config/roleman/config.toml`

```toml
default_identity = "work"
refresh_seconds = 300
hook_prompt = "always"

[[identities]]
name = "work"
start_url = "https://acme.awsapps.com/start"
sso_region = "us-east-1"
ignore_roles = ["ReadOnly"]

accounts = [
  { account_id = "123456789012", alias = "Platform", precedence = 10 },
  { account_id = "999999999999", ignored = true },
  { account_id = "123456789012", ignored_roles = ["Admin"] }
]
```

Notes:
- Higher `precedence` appears first.
- `hook_prompt` values: `always`, `outdated`, `never`.
- Use `--show-all` to bypass account/role filters for one run.

## Command Reference

```text
roleman [--sso-start-url <url>] [--sso-region <region>] [--account <name>] [--no-cache] [--show-all] [--refresh-seconds <n>] [--env-file <path>] [--print] [--config <path>]
roleman set|s [--account <name>]
roleman open|o [--account <name>]
roleman <sso-start-url>
roleman hook zsh|bash
roleman install-hook [--force] [--alias]
roleman unset|u
```

## Troubleshooting

Enable trace logs to a file (recommended because the selector UI redraws the terminal):

```sh
ROLEMAN_LOG_FILE=/tmp/roleman.log RUST_LOG=roleman=trace roleman --sso-start-url https://acme.awsapps.com/start --sso-region us-east-1
```

If you see hook warnings, reload your shell:

```sh
source ~/.zshrc
# or
source ~/.bashrc
```

## Development

```sh
cargo run -- --help
cargo build
cargo test
cargo clippy -- -D warnings
cargo deny check advisories bans sources
```
