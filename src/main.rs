use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

mod shell;

use crate::shell::{Shell, detect_shell_from_env, shell_for_name};
use clap::{Args, Parser, Subcommand, ValueEnum};
use roleman::{
    App, AppAction, AppOptions, Config,
    config::{HookPromptMode, SelectorSortMode},
    history, ui,
};
use tracing_subscriber::prelude::*;

#[derive(Debug, Parser)]
#[command(
    name = "roleman",
    about = "Select an AWS IAM Identity Center role and export temporary AWS credentials",
    long_about = "Roleman lets you pick an AWS IAM Identity Center (AWS SSO) account and role, then emits shell exports for temporary AWS credentials.\n\nUse `roleman` for interactive credential export, `roleman login` to ensure you have a valid IAM Identity Center session, `roleman list` to inspect available account and role combinations, `roleman open` to open the selected role in the AWS access portal, and `roleman hook`/`roleman install-hook` for shell integration.",
    disable_help_subcommand = true,
    after_help = "Examples:\n  roleman\n  roleman --account prod\n  roleman -q sandbox\n  roleman --no-cache --print\n  roleman --no-cache --close-auth-tab --focus-terminal-after-auth\n  roleman --sso-start-url https://acme.awsapps.com/start --sso-region us-east-1\n  roleman login\n  roleman login --account prod\n  roleman list\n  roleman list --format json\n  roleman open\n  roleman hook\n  roleman install-hook --alias"
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
        long = "sort",
        value_enum,
        value_name = "mode",
        help = "Selector sorting mode (overrides config selector_sort)"
    )]
    sort: Option<SortArg>,

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

    #[arg(
        long = "focus-terminal-after-auth",
        help = "After successful SSO auth, try to bring your terminal app back to front"
    )]
    focus_terminal_after_auth: bool,

    #[arg(
        long = "close-auth-tab",
        help = "After successful SSO auth, try to close the frontmost browser tab before refocusing terminal"
    )]
    close_auth_tab: bool,

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
        about = "Ensure there is a valid IAM Identity Center session",
        long_about = "Resolve the selected IAM Identity Center identity, reuse a valid cached SSO token if present, or trigger the AWS SSO login flow if not. This command exits after authentication succeeds or fails.",
        after_help = "Examples:\n  roleman login\n  roleman login prod\n  roleman login --account prod\n  roleman login --no-cache"
    )]
    Login(RunSubcommandArgs),
    #[command(
        alias = "ls",
        about = "List available account and role combinations",
        long_about = "Resolve the selected IAM Identity Center identity and print the available account/role combinations.\n\nThe default output is a text table. Use `--format json` for machine-readable output.",
        after_help = "Examples:\n  roleman list\n  roleman list prod\n  roleman list --account prod --show-all\n  roleman list --format json"
    )]
    List(ListArgs),
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
    #[command(
        about = "Inspect or clear local role selection history",
        long_about = "Print recent role selections used for dynamic ordering, or clear that history.",
        after_help = "Examples:\n  roleman history\n  roleman history --limit 20\n  roleman history clear"
    )]
    History(HistoryArgs),
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

#[derive(Debug, Args)]
struct ListArgs {
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
        long = "sort",
        value_enum,
        value_name = "mode",
        help = "Listing order (overrides config selector_sort)"
    )]
    sort: Option<SortArg>,

    #[arg(
        long = "refresh-seconds",
        help = "Polling interval in seconds while waiting for available roles"
    )]
    refresh_seconds: Option<u64>,

    #[arg(
        long = "focus-terminal-after-auth",
        help = "After successful SSO auth, try to bring your terminal app back to front"
    )]
    focus_terminal_after_auth: bool,

    #[arg(
        long = "close-auth-tab",
        help = "After successful SSO auth, try to close the frontmost browser tab before refocusing terminal"
    )]
    close_auth_tab: bool,

    #[arg(long = "config", help = "Path to config.toml")]
    config_path: Option<PathBuf>,

    #[arg(
        value_name = "account",
        id = "command_account",
        help = "Configured identity name to use instead of default_identity"
    )]
    command_account: Option<String>,

    #[arg(
        long,
        value_enum,
        default_value_t = OutputFormatArg::Text,
        help = "Output format"
    )]
    format: OutputFormatArg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SortArg {
    Dynamic,
    Alphabetical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
enum OutputFormatArg {
    #[default]
    Text,
    Json,
}

impl From<SortArg> for SelectorSortMode {
    fn from(value: SortArg) -> Self {
        match value {
            SortArg::Dynamic => SelectorSortMode::Dynamic,
            SortArg::Alphabetical => SelectorSortMode::Alphabetical,
        }
    }
}

#[derive(Debug, Args)]
struct HistoryArgs {
    #[command(subcommand)]
    command: Option<HistorySubcommand>,

    #[arg(long, default_value_t = 50, help = "Maximum history rows to print")]
    limit: usize,

    #[arg(long, help = "Print history entries as JSON")]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum HistorySubcommand {
    #[command(about = "Clear local role selection history")]
    Clear,
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
        Some(CliCommand::History(args)) => {
            if let Err(err) = handle_history(args) {
                eprintln!("error: {err}");
                std::process::exit(2);
            }
            return;
        }
        _ => {}
    }

    let options = build_app_options(&cli);
    if let Some(CliCommand::List(args)) = &cli.command {
        if let Err(err) = handle_list(args, options) {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
        return;
    }

    if matches!(options.action, AppAction::Set | AppAction::Open) {
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
        Some(CliCommand::Login(args)) => {
            let common = merge_common_args(&cli.common, &args.common);
            app_options_from_parts(&common, AppAction::Login, args.account.clone())
        }
        Some(CliCommand::List(args)) => {
            let common = merge_list_args(&cli.common, args);
            app_options_from_parts(&common, AppAction::List, args.command_account.clone())
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
        focus_terminal_after_auth: common.focus_terminal_after_auth,
        close_auth_tab: common.close_auth_tab,
        account: common.account.clone().or(positional_account),
        show_all: common.show_all,
        initial_query: common.initial_query.clone(),
        selector_sort: common.sort.map(Into::into),
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
        sort: child.sort.or(parent.sort),
        initial_query: child
            .initial_query
            .clone()
            .or_else(|| parent.initial_query.clone()),
        refresh_seconds: child.refresh_seconds.or(parent.refresh_seconds),
        env_file: child.env_file.clone().or_else(|| parent.env_file.clone()),
        print_env: child.print_env || parent.print_env,
        focus_terminal_after_auth: child.focus_terminal_after_auth
            || parent.focus_terminal_after_auth,
        close_auth_tab: child.close_auth_tab || parent.close_auth_tab,
        config_path: child
            .config_path
            .clone()
            .or_else(|| parent.config_path.clone()),
    }
}

fn merge_list_args(parent: &CommonArgs, args: &ListArgs) -> CommonArgs {
    CommonArgs {
        sso_start_url: args
            .sso_start_url
            .clone()
            .or_else(|| parent.sso_start_url.clone()),
        sso_region: args
            .sso_region
            .clone()
            .or_else(|| parent.sso_region.clone()),
        account: args.account.clone().or_else(|| parent.account.clone()),
        no_cache: args.no_cache || parent.no_cache,
        show_all: args.show_all || parent.show_all,
        sort: args.sort.or(parent.sort),
        initial_query: None,
        refresh_seconds: args.refresh_seconds.or(parent.refresh_seconds),
        env_file: None,
        print_env: false,
        focus_terminal_after_auth: args.focus_terminal_after_auth
            || parent.focus_terminal_after_auth,
        close_auth_tab: args.close_auth_tab || parent.close_auth_tab,
        config_path: args
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

fn handle_history(args: &HistoryArgs) -> Result<(), String> {
    match args.command {
        Some(HistorySubcommand::Clear) => {
            history::clear_entries().map_err(|err| err.to_string())?;
            let path = history::history_path().map_err(|err| err.to_string())?;
            println!("Cleared history at {}", path.display());
        }
        None => {
            let entries = history::recent_entries(args.limit).map_err(|err| err.to_string())?;
            if entries.is_empty() {
                println!("No role history recorded yet.");
                return Ok(());
            }
            if args.json {
                let json = serde_json::to_string_pretty(&entries).map_err(|err| err.to_string())?;
                println!("{json}");
            } else {
                print_history_table(&entries);
            }
        }
    }

    Ok(())
}

fn handle_list(args: &ListArgs, options: AppOptions) -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new().map_err(|err| err.to_string())?;
    let roles = runtime
        .block_on(App::new(options).list_roles())
        .map_err(|err| err.to_string())?;
    match args.format {
        OutputFormatArg::Text => {
            if roles.is_empty() {
                println!("No roles available.");
            } else {
                print_role_table(&roles);
            }
        }
        OutputFormatArg::Json => {
            let json = serde_json::to_string_pretty(&roles).map_err(|err| err.to_string())?;
            println!("{json}");
        }
    }
    Ok(())
}

fn print_history_table(entries: &[history::HistoryEntry]) {
    let headers = ["Timestamp", "Identity", "Account", "Role", "Cwd"];
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|entry| {
            vec![
                history::format_timestamp(entry.selected_at_unix),
                entry.identity.clone(),
                format!("{} ({})", entry.account_name, entry.account_id),
                entry.role_name.clone(),
                entry
                    .cwd
                    .as_deref()
                    .map(compact_home_path)
                    .unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();
    println!("{}", format_table(&headers, &rows));
}

fn print_role_table(roles: &[roleman::RoleChoice]) {
    let headers = ["Account", "Account ID", "Role"];
    let rows: Vec<Vec<String>> = roles
        .iter()
        .map(|role| {
            vec![
                role.account_name.clone(),
                role.account_id.clone(),
                role.role_name.clone(),
            ]
        })
        .collect();
    println!("{}", format_table(&headers, &rows));
}

fn format_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    debug_assert!(rows.iter().all(|row| row.len() == headers.len()));

    let mut widths = headers
        .iter()
        .map(|header| header.len())
        .collect::<Vec<_>>();
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            widths[index] = widths[index].max(value.len());
        }
    }

    let mut output = String::new();
    append_table_line(&mut output, headers.iter().copied(), &widths);
    output.push('\n');
    append_table_separator(&mut output, &widths);

    for row in rows {
        output.push('\n');
        append_table_line(&mut output, row.iter().map(String::as_str), &widths);
    }

    output
}

fn append_table_line<'a, I>(output: &mut String, cells: I, widths: &[usize])
where
    I: IntoIterator<Item = &'a str>,
{
    for (index, cell) in cells.into_iter().enumerate() {
        if index > 0 {
            output.push_str("  ");
        }
        let _ = write!(output, "{cell:<width$}", width = widths[index]);
    }
}

fn append_table_separator(output: &mut String, widths: &[usize]) {
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            output.push_str("  ");
        }
        for _ in 0..*width {
            output.push('-');
        }
    }
}

fn compact_home_path(path: &str) -> String {
    let Ok(home) = std::env::var("HOME") else {
        return path.to_string();
    };
    let home = Path::new(&home);
    let path = Path::new(path);
    let Ok(stripped) = path.strip_prefix(home) else {
        return path.display().to_string();
    };
    if stripped.as_os_str().is_empty() {
        "~".to_string()
    } else {
        format!("~/{}", stripped.display())
    }
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
    use super::{Cli, CliCommand, HistorySubcommand, OutputFormatArg, build_app_options};
    use clap::Parser;
    use roleman::{AppAction, RoleChoice};
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock_env() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().expect("failed to lock env mutex")
    }

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
    fn parses_login_with_positional_account() {
        let cli =
            Cli::try_parse_from(["roleman", "login", "prod"]).expect("expected login to parse");
        let options = build_app_options(&cli);
        assert_eq!(options.account.as_deref(), Some("prod"));
        assert!(matches!(options.action, AppAction::Login));
    }

    #[test]
    fn parses_list_with_positional_account() {
        let cli = Cli::try_parse_from(["roleman", "list", "prod"]).expect("expected list to parse");
        let options = build_app_options(&cli);
        assert_eq!(options.account.as_deref(), Some("prod"));
        assert!(matches!(options.action, AppAction::List));
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

    #[test]
    fn parses_history_clear_command() {
        let cli =
            Cli::try_parse_from(["roleman", "history", "clear"]).expect("expected history clear");
        match cli.command {
            Some(CliCommand::History(args)) => {
                assert!(matches!(args.command, Some(HistorySubcommand::Clear)));
            }
            _ => panic!("expected history command"),
        }
    }

    #[test]
    fn parses_history_json_flag() {
        let cli = Cli::try_parse_from(["roleman", "history", "--json"])
            .expect("expected history json flag parse");
        match cli.command {
            Some(CliCommand::History(args)) => {
                assert!(args.json);
                assert!(args.command.is_none());
            }
            _ => panic!("expected history command"),
        }
    }

    #[test]
    fn parses_list_json_format_flag() {
        let cli = Cli::try_parse_from(["roleman", "list", "--format", "json"])
            .expect("expected list format parse");
        match cli.command {
            Some(CliCommand::List(args)) => assert_eq!(args.format, OutputFormatArg::Json),
            _ => panic!("expected list command"),
        }
    }

    #[test]
    fn parses_sort_flag() {
        let cli = Cli::try_parse_from(["roleman", "--sort", "alphabetical"])
            .expect("expected sort flag parse");
        let options = build_app_options(&cli);
        assert_eq!(
            options.selector_sort,
            Some(roleman::config::SelectorSortMode::Alphabetical)
        );
    }

    #[test]
    fn parses_focus_terminal_after_auth_flag() {
        let cli = Cli::try_parse_from(["roleman", "--focus-terminal-after-auth"])
            .expect("expected --focus-terminal-after-auth to parse");
        let options = build_app_options(&cli);
        assert!(options.focus_terminal_after_auth);
        assert!(!options.close_auth_tab);
    }

    #[test]
    fn parses_close_auth_tab_flag() {
        let cli = Cli::try_parse_from(["roleman", "set", "--close-auth-tab"])
            .expect("expected --close-auth-tab to parse");
        let options = build_app_options(&cli);
        assert!(options.close_auth_tab);
        assert!(matches!(options.action, AppAction::Set));
    }

    #[test]
    fn compacts_home_prefix_to_tilde() {
        let _lock = lock_env();
        let previous = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", "/Users/alberto");
        }
        assert_eq!(super::compact_home_path("/Users/alberto"), "~");
        assert_eq!(
            super::compact_home_path("/Users/alberto/Source/roleman"),
            "~/Source/roleman"
        );
        assert_eq!(
            super::compact_home_path("/tmp/roleman"),
            "/tmp/roleman".to_string()
        );
        unsafe {
            if let Some(value) = previous {
                std::env::set_var("HOME", value);
            } else {
                std::env::remove_var("HOME");
            }
        }
    }

    #[test]
    fn formats_role_table() {
        let roles = vec![
            RoleChoice {
                account_id: "123456789012".into(),
                account_name: "Platform".into(),
                role_name: "Admin".into(),
            },
            RoleChoice {
                account_id: "210987654321".into(),
                account_name: "Sandbox".into(),
                role_name: "ReadOnly".into(),
            },
        ];

        let headers = ["Account", "Account ID", "Role"];
        let rows = roles
            .iter()
            .map(|role| {
                vec![
                    role.account_name.clone(),
                    role.account_id.clone(),
                    role.role_name.clone(),
                ]
            })
            .collect::<Vec<_>>();

        let table = super::format_table(&headers, &rows);

        assert!(table.contains("Account   Account ID    Role"));
        assert!(table.contains("Platform  123456789012  Admin"));
        assert!(table.contains("Sandbox   210987654321  ReadOnly"));
    }
}
