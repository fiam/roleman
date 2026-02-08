use std::fs::OpenOptions;
use std::io::IsTerminal;
use std::path::PathBuf;

mod shell;

use crate::shell::{Shell, detect_shell_from_env, shell_for_name};
use clap::{Args, Parser, Subcommand};
use roleman::{App, AppAction, AppOptions, Config, config::HookPromptMode, ui};
use tracing_subscriber::prelude::*;

#[derive(Debug, Parser)]
#[command(
    name = "roleman",
    about = "Select an AWS IAM Identity Center role and export temporary AWS credentials",
    long_about = "Roleman lets you pick an AWS IAM Identity Center (AWS SSO) account and role, then emits shell exports for temporary AWS credentials.\n\nUse `roleman` for interactive credential export, `roleman open` to open the selected role in the AWS access portal, and `roleman hook`/`roleman install-hook` for shell integration.",
    disable_help_subcommand = true,
    after_help = "Examples:\n  roleman\n  roleman --account prod\n  roleman -q sandbox\n  roleman --no-cache --print\n  roleman --sso-start-url https://acme.awsapps.com/start --sso-region us-east-1\n  roleman open\n  roleman hook\n  roleman install-hook --alias"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,

    #[command(flatten)]
    common: CommonArgs,
}

#[derive(Debug, Args, Clone, Default)]
struct CommonArgs {
    #[arg(
        long = "sso-start-url",
        help = "IAM Identity Center start URL to use for this run"
    )]
    sso_start_url: Option<String>,

    #[arg(
        long = "sso-region",
        help = "IAM Identity Center region (for example: us-east-1)"
    )]
    sso_region: Option<String>,

    #[arg(
        short = 'a',
        long = "account",
        help = "Configured identity name to use instead of default_identity"
    )]
    account: Option<String>,

    #[arg(
        long = "no-cache",
        help = "Ignore role/token caches and force refresh or sign-in"
    )]
    no_cache: bool,

    #[arg(
        long = "show-all",
        help = "Ignore configured account/role filters for this run"
    )]
    show_all: bool,

    #[arg(
        short = 'q',
        long = "query",
        value_name = "term",
        help = "Initial query term for account/role selection"
    )]
    initial_query: Option<String>,

    #[arg(
        long = "refresh-seconds",
        help = "Polling interval in seconds while waiting for available roles"
    )]
    refresh_seconds: Option<u64>,

    #[arg(
        long = "env-file",
        help = "Write env exports to this file (used by shell hooks)"
    )]
    env_file: Option<PathBuf>,

    #[arg(
        long = "print",
        help = "Print env exports to stdout even when --env-file is set"
    )]
    print_env: bool,

    #[arg(long = "config", help = "Path to config.toml")]
    config_path: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    #[command(
        alias = "s",
        about = "Select a role and emit AWS credential exports",
        long_about = "Launch the role selector and emit AWS credential exports for the chosen role.\n\nThis is equivalent to running `roleman` without a subcommand.",
        after_help = "Examples:\n  roleman set\n  roleman set prod\n  roleman set --account prod\n  roleman set -q sandbox"
    )]
    Set(RunSubcommandArgs),
    #[command(
        alias = "o",
        about = "Select a role and open it in the AWS access portal",
        long_about = "Launch the role selector and open the selected account/role directly in the AWS access portal.",
        after_help = "Examples:\n  roleman open\n  roleman open prod\n  roleman open --account prod\n  roleman open -q prod-admin"
    )]
    Open(RunSubcommandArgs),
    #[command(
        about = "Print shell hook code for shell integration",
        long_about = "Print the shell hook script to stdout. If no shell is provided, roleman auto-detects it from $SHELL.",
        after_help = "Examples:\n  eval \"$(roleman hook)\"\n  eval \"$(roleman hook zsh)\"\n  roleman hook fish | source"
    )]
    Hook {
        #[arg(
            value_name = "shell",
            help = "Shell name (bash, zsh, or fish). Defaults to auto-detect from $SHELL"
        )]
        shell: Option<String>,
    },
    #[command(
        name = "install-hook",
        about = "Install shell hook into your shell startup file",
        long_about = "Detect your current shell and append the roleman hook loader to the corresponding shell startup file.\n\nSupported shells: bash, zsh, fish.",
        after_help = "Examples:\n  roleman install-hook\n  roleman install-hook --alias\n  roleman install-hook --force --alias"
    )]
    InstallHook {
        #[arg(long, help = "Remove existing roleman hook lines before reinstalling")]
        force: bool,
        #[arg(long, help = "Also install a short alias (`rl`) for `roleman`")]
        alias: bool,
    },
    #[command(
        alias = "u",
        about = "Unset roleman-managed AWS environment variables",
        long_about = "Prints shell commands to unset AWS environment variables managed by roleman.\n\nWhen running under a shell hook, writes the unset command to the hook env file so your current shell is updated."
    )]
    Unset,
}

#[derive(Debug, Args)]
struct RunSubcommandArgs {
    #[command(flatten)]
    common: CommonArgs,

    #[arg(
        value_name = "account",
        id = "command_account",
        help = "Configured identity name to use instead of default_identity"
    )]
    account: Option<String>,
}

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

    let cli = Cli::parse();

    match &cli.command {
        Some(CliCommand::Hook { shell }) => {
            let shell = match resolve_hook_shell(shell.as_deref()) {
                Ok(shell) => shell,
                Err(err) => {
                    eprintln!("error: {err}");
                    std::process::exit(2);
                }
            };
            print_hook(shell);
            return;
        }
        Some(CliCommand::InstallHook { force, alias }) => {
            if let Err(err) = install_hook(*force, *alias) {
                eprintln!("error: {err}");
                std::process::exit(2);
            }
            return;
        }
        Some(CliCommand::Unset) => {
            handle_unset();
            return;
        }
        _ => {}
    }

    let options = build_app_options(&cli);
    maybe_prompt_install_hook(options.config_path.as_deref());

    let runtime = tokio::runtime::Runtime::new().expect("failed to start runtime");
    let result = runtime.block_on(App::new(options).run());
    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }

    drop(_guard);
}

fn build_app_options(cli: &Cli) -> AppOptions {
    match &cli.command {
        Some(CliCommand::Set(args)) => {
            let common = merge_common_args(&cli.common, &args.common);
            app_options_from_parts(&common, AppAction::Set, args.account.clone())
        }
        Some(CliCommand::Open(args)) => {
            let common = merge_common_args(&cli.common, &args.common);
            app_options_from_parts(&common, AppAction::Open, args.account.clone())
        }
        _ => app_options_from_parts(&cli.common, AppAction::Set, None),
    }
}

fn app_options_from_parts(
    common: &CommonArgs,
    action: AppAction,
    positional_account: Option<String>,
) -> AppOptions {
    AppOptions {
        start_url: common.sso_start_url.clone(),
        sso_region: common.sso_region.clone(),
        refresh_seconds: common.refresh_seconds,
        config_path: common.config_path.clone(),
        ignore_cache: common.no_cache,
        env_file: common.env_file.clone(),
        print_env: common.print_env,
        account: common.account.clone().or(positional_account),
        show_all: common.show_all,
        initial_query: common.initial_query.clone(),
        action,
    }
}

fn merge_common_args(parent: &CommonArgs, child: &CommonArgs) -> CommonArgs {
    CommonArgs {
        sso_start_url: child
            .sso_start_url
            .clone()
            .or_else(|| parent.sso_start_url.clone()),
        sso_region: child
            .sso_region
            .clone()
            .or_else(|| parent.sso_region.clone()),
        account: child.account.clone().or_else(|| parent.account.clone()),
        no_cache: child.no_cache || parent.no_cache,
        show_all: child.show_all || parent.show_all,
        initial_query: child
            .initial_query
            .clone()
            .or_else(|| parent.initial_query.clone()),
        refresh_seconds: child.refresh_seconds.or(parent.refresh_seconds),
        env_file: child.env_file.clone().or_else(|| parent.env_file.clone()),
        print_env: child.print_env || parent.print_env,
        config_path: child
            .config_path
            .clone()
            .or_else(|| parent.config_path.clone()),
    }
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

#[cfg(test)]
mod tests {
    use super::{Cli, CliCommand, build_app_options};
    use clap::Parser;
    use roleman::AppAction;

    #[test]
    fn parses_hook_without_shell_argument() {
        let cli = Cli::try_parse_from(["roleman", "hook"]).expect("expected hook to parse");
        match cli.command {
            Some(CliCommand::Hook { shell }) => assert!(shell.is_none()),
            _ => panic!("expected hook command"),
        }
    }

    #[test]
    fn parses_set_alias_with_positional_account() {
        let cli =
            Cli::try_parse_from(["roleman", "s", "prod"]).expect("expected set alias to parse");
        let options = build_app_options(&cli);
        assert_eq!(options.account.as_deref(), Some("prod"));
        assert!(matches!(options.action, AppAction::Set));
    }

    #[test]
    fn parses_open_alias_with_flag_account() {
        let cli = Cli::try_parse_from(["roleman", "o", "--account", "prod"])
            .expect("expected open alias to parse");
        let options = build_app_options(&cli);
        assert_eq!(options.account.as_deref(), Some("prod"));
        assert!(matches!(options.action, AppAction::Open));
    }

    #[test]
    fn rejects_positional_start_url() {
        let result = Cli::try_parse_from(["roleman", "https://acme.awsapps.com/start"]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_initial_query_long_flag() {
        let cli = Cli::try_parse_from(["roleman", "set", "--query", "sandbox-admin"])
            .expect("expected --query to parse");
        let options = build_app_options(&cli);
        assert_eq!(options.initial_query.as_deref(), Some("sandbox-admin"));
        assert!(matches!(options.action, AppAction::Set));
    }

    #[test]
    fn parses_initial_query_short_flag() {
        let cli =
            Cli::try_parse_from(["roleman", "open", "-q", "prod-admin"]).expect("expected -q");
        let options = build_app_options(&cli);
        assert_eq!(options.initial_query.as_deref(), Some("prod-admin"));
        assert!(matches!(options.action, AppAction::Open));
    }

    #[test]
    fn rejects_search_alias_after_standardizing_query_flag() {
        let cli = Cli::try_parse_from(["roleman", "--search", "sandbox"]);
        assert!(cli.is_err());
    }
}
