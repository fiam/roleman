use std::fs::OpenOptions;
use std::path::PathBuf;

use roleman::{App, AppAction, AppOptions};
use tracing_subscriber::prelude::*;

fn main() {
    let env_filter = tracing_subscriber::EnvFilter::from_default_env();
    let log_file = std::env::var("ROLEMAN_LOG_FILE").ok();
    let _guard = if let Some(path) = log_file {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("failed to open ROLEMAN_LOG_FILE");
        let (writer, guard) = tracing_appender::non_blocking(file);
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .with(tracing_subscriber::fmt::layer().with_writer(writer))
            .init();
        Some(guard)
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
        None
    };

    let args_vec = std::env::args().skip(1).collect::<Vec<_>>();
    let subcommand = args_vec.first().map(|v| v.as_str());
    let is_hook = matches!(subcommand, Some("hook"));
    let is_unset = matches!(subcommand, Some("unset") | Some("u"));
    let is_set = matches!(subcommand, Some("set") | Some("s"));
    let is_open = matches!(subcommand, Some("open") | Some("o"));
    if is_hook {
        let shell = args_vec.get(1).cloned().unwrap_or_default();
        if shell == "zsh" {
            print_zsh_hook();
            return;
        }
        eprintln!("unsupported shell hook: {shell}");
        std::process::exit(2);
    }
    if is_unset {
        handle_unset();
        return;
    }
    let mut options = AppOptions::default();
    if is_open {
        options.action = AppAction::Open;
    }
    let mut index = if is_set || is_open { 1 } else { 0 };
    if (is_set || is_open)
        && let Some(value) = args_vec.get(1)
        && !value.starts_with('-')
    {
        options.account = Some(value.clone());
        index = 2;
    }
    while index < args_vec.len() {
        let arg = &args_vec[index];
        match arg.as_str() {
            "--sso-start-url" => {
                index += 1;
                options.start_url = args_vec.get(index).cloned();
                if options.start_url.is_none() {
                    exit_usage("missing value for --sso-start-url");
                }
            }
            "--no-cache" => {
                options.ignore_cache = true;
            }
            "--show-all" => {
                options.show_all = true;
            }
            "--sso-region" => {
                index += 1;
                options.sso_region = args_vec.get(index).cloned();
                if options.sso_region.is_none() {
                    exit_usage("missing value for --sso-region");
                }
            }
            "-a" | "--account" => {
                index += 1;
                options.account = args_vec.get(index).cloned();
                if options.account.is_none() {
                    exit_usage("missing value for --account");
                }
            }
            "--refresh-seconds" => {
                index += 1;
                let value = args_vec.get(index).cloned().unwrap_or_default();
                let parsed = value.parse::<u64>().ok();
                if parsed.is_none() {
                    exit_usage("invalid value for --refresh-seconds");
                }
                options.refresh_seconds = parsed;
            }
            "--env-file" => {
                index += 1;
                let value = args_vec.get(index).cloned().unwrap_or_default();
                if value.is_empty() {
                    exit_usage("missing value for --env-file");
                }
                options.env_file = Some(PathBuf::from(value));
            }
            "--print" => {
                options.print_env = true;
            }
            "--config" => {
                index += 1;
                let value = args_vec.get(index).cloned().unwrap_or_default();
                if value.is_empty() {
                    exit_usage("missing value for --config");
                }
                options.config_path = Some(PathBuf::from(value));
            }
            "-h" | "--help" => {
                print_usage();
                return;
            }
            _ => {
                if options.start_url.is_none() {
                    options.start_url = Some(arg.to_string());
                } else {
                    exit_usage("unexpected argument");
                }
            }
        }
        index += 1;
    }

    let runtime = tokio::runtime::Runtime::new().expect("failed to start runtime");
    let result = runtime.block_on(App::new(options).run());
    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }

    drop(_guard);
}

fn print_usage() {
    eprintln!(
        "usage: roleman [--sso-start-url <url>] [--sso-region <region>] [--account <name>] [--no-cache] [--show-all] [--refresh-seconds <n>] [--env-file <path>] [--print] [--config <path>]\n       roleman set|s [--account <name>]\n       roleman open|o [--account <name>]\n       roleman <sso-start-url>\n       roleman hook zsh\n       roleman unset|u"
    );
}

fn print_zsh_hook() {
    println!(
        r##"export _ROLEMAN_HOOK_ENV="${{XDG_STATE_HOME:-$HOME/.local/state}}/roleman/env-${{TTY//\//_}}"
roleman() {{
  command roleman --env-file "$_ROLEMAN_HOOK_ENV" "$@"
}}
_roleman_precmd() {{
  if [[ -f "$_ROLEMAN_HOOK_ENV" ]]; then
    source "$_ROLEMAN_HOOK_ENV"
    rm -f "$_ROLEMAN_HOOK_ENV"
  fi
}}
autoload -Uz add-zsh-hook
add-zsh-hook precmd _roleman_precmd"##
    );
}

fn print_unset_exports() {
    println!(
        "unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN AWS_CREDENTIAL_EXPIRATION AWS_DEFAULT_REGION AWS_REGION AWS_PROFILE"
    );
}

fn handle_unset() {
    if let Ok(path) = std::env::var("_ROLEMAN_HOOK_ENV")
        && !path.is_empty()
    {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, unset_payload());
        return;
    }
    print_unset_exports();
}

fn unset_payload() -> &'static str {
    "unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN AWS_CREDENTIAL_EXPIRATION AWS_DEFAULT_REGION AWS_REGION AWS_PROFILE\n"
}

fn exit_usage(message: &str) -> ! {
    eprintln!("error: {message}");
    print_usage();
    std::process::exit(2);
}
