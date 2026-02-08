use std::fs::OpenOptions;
use std::io::IsTerminal;
use std::path::PathBuf;

mod shell;

use crate::shell::{Shell, detect_shell_from_env, shell_for_name};
use roleman::{App, AppAction, AppOptions, Config, config::HookPromptMode, ui};
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
    let is_install_hook = matches!(subcommand, Some("install-hook"));
    let is_unset = matches!(subcommand, Some("unset") | Some("u"));
    let is_set = matches!(subcommand, Some("set") | Some("s"));
    let is_open = matches!(subcommand, Some("open") | Some("o"));
    if is_hook {
        let shell_name = args_vec.get(1).map(|value| value.as_str());
        let shell = match resolve_hook_shell(shell_name) {
            Ok(shell) => shell,
            Err(err) => {
                eprintln!("error: {err}");
                std::process::exit(2);
            }
        };
        print_hook(shell);
        return;
    }
    if is_install_hook {
        let (force, alias) = parse_install_hook_args(&args_vec[1..]);
        if let Err(err) = install_hook(force, alias) {
            eprintln!("error: {err}");
            std::process::exit(2);
        }
        return;
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

    if !is_hook && !is_install_hook {
        maybe_prompt_install_hook(options.config_path.as_deref());
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
        "usage: roleman [--sso-start-url <url>] [--sso-region <region>] [--account <name>] [--no-cache] [--show-all] [--refresh-seconds <n>] [--env-file <path>] [--print] [--config <path>]\n       roleman set|s [--account <name>]\n       roleman open|o [--account <name>]\n       roleman <sso-start-url>\n       roleman hook [zsh|bash|fish]\n       roleman install-hook [--force] [--alias]\n       roleman unset|u"
    );
}

fn print_hook(shell: &dyn Shell) {
    println!("{}", shell.hook_snippet());
}

fn resolve_hook_shell(shell_name: Option<&str>) -> Result<&'static dyn Shell, String> {
    if let Some(name) = shell_name {
        return shell_for_name(name).ok_or_else(|| format!("unsupported shell hook: {name}"));
    }
    detect_shell_from_env().ok_or_else(|| {
        "failed to auto-detect shell (set SHELL to bash, zsh, or fish, or pass `roleman hook <shell>`)"
            .to_string()
    })
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

fn parse_install_hook_args(args: &[String]) -> (bool, bool) {
    let mut force = false;
    let mut alias = false;
    for arg in args {
        match arg.as_str() {
            "--force" => force = true,
            "--alias" => alias = true,
            _ => {
                exit_usage("invalid argument for install-hook");
            }
        }
    }
    (force, alias)
}

fn install_hook(force: bool, alias: bool) -> Result<(), String> {
    let shell = detect_shell_from_env().ok_or("unsupported shell (expected bash, zsh, or fish)")?;
    let path = shell.rc_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let mut contents = std::fs::read_to_string(&path).unwrap_or_default();
    let install_line = shell.install_line();
    if has_active_hook(&contents, &install_line) {
        if !force {
            return Err("hook already installed (use --force to overwrite)".into());
        }
        contents = remove_hook_lines(&contents);
    }
    let mut block = String::new();
    block.push('\n');
    block.push_str(&install_line);
    if alias {
        block.push('\n');
        block.push_str(shell.alias_line());
    }
    block.push('\n');
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(&block);
    std::fs::write(&path, contents).map_err(|err| err.to_string())?;
    println!("Installed hook into {}", path.display());
    Ok(())
}

fn remove_hook_lines(contents: &str) -> String {
    contents
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed != "alias rl='roleman'"
                && trimmed != "alias rl roleman"
                && trimmed != "export _ROLEMAN_HOOK_VERSION=1"
                && !trimmed.starts_with("eval \"$(roleman hook ")
                && !trimmed.starts_with("roleman hook ")
                && !trimmed.contains("_ROLEMAN_HOOK_ENV")
                && !trimmed.contains("_ROLEMAN_HOOK_VERSION")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn maybe_prompt_install_hook(config_path: Option<&std::path::Path>) {
    let (mut config, config_path) = match Config::load(config_path) {
        Ok((config, path)) => (config, path),
        Err(err) => {
            ui::print_warn(&format!("Failed to load config for hook prompt: {err}"));
            (Config::default(), default_config_path())
        }
    };
    let mode = hook_prompt_mode(&config);
    if matches!(mode, HookPromptMode::Never) {
        return;
    }
    if std::env::var("_ROLEMAN_HOOK_VERSION").is_ok() {
        return;
    }
    let Some(shell) = detect_shell_from_env() else {
        return;
    };
    let Ok(path) = shell.rc_path() else {
        return;
    };
    let install_line = shell.install_line();
    if std::env::var("_ROLEMAN_HOOK_ENV").is_ok() {
        let reload_cmd = shell.reload_command(&path);
        ui::print_warn(&format!(
            "Shell hook looks outdated. Please reload your shell: {reload_cmd}"
        ));
        return;
    }
    let contents = std::fs::read_to_string(&path).unwrap_or_default();
    if has_active_hook(&contents, &install_line) {
        let reload_cmd = shell.reload_command(&path);
        ui::print_warn(&format!(
            "Shell hook is installed but not active. Reload your shell: {reload_cmd}"
        ));
        return;
    }
    if matches!(mode, HookPromptMode::Outdated) {
        return;
    }
    if !std::io::stdin().is_terminal() {
        return;
    }
    ui::print_line(&ui::hint("Shell hook isn’t installed."));
    ui::print_line(&ui::hint(&format!(
        "Want me to add this to {}?",
        path.display()
    )));
    ui::print_line("");
    ui::print_line(&install_line);
    ui::print_line("");
    if !prompt_yes_no("Would you like to install it? [y/N] ") {
        if prompt_yes_no("Don’t ask about the hook again? [y/N] ") {
            config.hook_prompt = Some(HookPromptMode::Never);
            config.prompt_for_hook = None;
            if let Err(err) = config.save(&config_path) {
                ui::print_warn(&format!("Failed to save config: {err}"));
            }
        }
        return;
    }
    let alias = prompt_yes_no("Also add alias rl=roleman? [y/N] ");
    if let Err(err) = install_hook(false, alias) {
        eprintln!("error: {err}");
    }
}

fn default_config_path() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        std::path::PathBuf::from(dir)
            .join("roleman")
            .join("config.toml")
    } else if let Ok(home) = std::env::var("HOME") {
        std::path::PathBuf::from(home)
            .join(".config")
            .join("roleman")
            .join("config.toml")
    } else {
        std::path::PathBuf::from("roleman-config.toml")
    }
}

fn prompt_yes_no(prompt: &str) -> bool {
    use std::io::{self, Write};
    let mut stdout = io::stdout();
    if stdout.write_all(prompt.as_bytes()).is_err() {
        return false;
    }
    if stdout.flush().is_err() {
        return false;
    }
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

fn has_active_hook(contents: &str, install_line: &str) -> bool {
    contents.lines().any(|line| {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return false;
        }
        trimmed.contains("_ROLEMAN_HOOK_VERSION")
            || trimmed.contains("_ROLEMAN_HOOK_ENV")
            || trimmed.contains(install_line)
    })
}

fn hook_prompt_mode(config: &Config) -> HookPromptMode {
    if let Some(mode) = config.hook_prompt {
        return mode;
    }
    match config.prompt_for_hook {
        Some(false) => HookPromptMode::Never,
        _ => HookPromptMode::Always,
    }
}
