pub mod config;
mod credentials_cache;
mod desktop;
mod error;
pub mod history;
mod model;
pub mod provider;
mod roles_cache;
mod tui;
pub mod ui;

pub use crate::config::Config;
use crate::config::{SelectorSortMode, SsoIdentity};
pub use crate::error::{Error, Result};
pub use crate::model::RoleChoice;
pub use crate::provider::AccessScope;
use crate::provider::{CloudProvider, PostLoginActions, ProviderCredentials, ProviderSession};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use tracing::debug;

pub struct App {
    options: AppOptions,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum AppAction {
    #[default]
    Set,
    Open,
    Login,
    List,
}

#[derive(Debug, Default)]
pub struct AppOptions {
    pub start_url: Option<String>,
    pub sso_region: Option<String>,
    pub refresh_seconds: Option<u64>,
    pub config_path: Option<PathBuf>,
    pub ignore_cache: bool,
    pub env_file: Option<PathBuf>,
    pub print_env: bool,
    pub focus_terminal_after_auth: bool,
    pub close_auth_tab: bool,
    pub account: Option<String>,
    pub show_all: bool,
    pub initial_query: Option<String>,
    pub selector_sort: Option<SelectorSortMode>,
    pub action: AppAction,
    pub scope: AccessScope,
    /// Skip the interactive confirmation before roleman creates a cloud resource.
    pub assume_yes: bool,
}

impl App {
    pub fn new(options: AppOptions) -> Self {
        Self { options }
    }

    pub async fn list_roles(&self) -> Result<Vec<RoleChoice>> {
        let (mut config, config_path) = Config::load(self.options.config_path.as_deref())?;
        let config_exists = config_path.exists();
        let identity = resolve_identity(&self.options, &mut config, &config_path, config_exists)?;
        let provider = provider::for_identity(&identity)?;
        Ok(self
            .prepare_visible_roles(provider.as_ref(), &identity)
            .await?
            .visible)
    }

    /// Scan for roleman-created cloud resources and remove them.
    ///
    /// By default uses the ambient credentials (whatever is active in the shell) and operates
    /// on the account you are currently in. With `all`, it uses the SSO session to sweep every
    /// reachable account (slower, mints credentials per account). With `dry_run`, lists without
    /// deleting.
    pub async fn cleanup_roles(&self, dry_run: bool, assume_yes: bool, all: bool) -> Result<()> {
        let (mut config, config_path) = Config::load(self.options.config_path.as_deref())?;
        let config_exists = config_path.exists();
        let identity = resolve_identity(&self.options, &mut config, &config_path, config_exists)?;
        let provider = provider::for_identity(&identity)?;

        if all {
            return self
                .cleanup_roles_all(provider.as_ref(), &config, dry_run, assume_yes)
                .await;
        }

        // Identify the current account from ambient credentials (works even from a read-only
        // shell), but do the IAM work with fresh privileged credentials minted via SSO.
        let account = provider.current_account().await?;
        let post_login_actions = resolve_post_login_actions(&self.options, &config);
        let session = provider
            .ensure_session(self.options.ignore_cache, post_login_actions)
            .await?;

        eprintln!(
            "{}",
            ui::info(&format!(
                "Scanning account {account} for roleman-managed resources..."
            ))
        );
        let resources = provider
            .list_managed_resources_in(session.as_ref(), &account)
            .await?;
        if resources.is_empty() {
            eprintln!(
                "{}",
                ui::info("No roleman-managed resources found in this account.")
            );
            return Ok(());
        }
        for resource in &resources {
            eprintln!("  {} {} — {}", resource.kind, resource.id, resource.detail);
        }
        if dry_run {
            eprintln!("{}", ui::info("Dry run: nothing was deleted."));
            return Ok(());
        }
        if !assume_yes
            && !prompt_yes_no(&format!(
                "Delete {} resource(s) in account {account}? [y/N] ",
                resources.len()
            ))?
        {
            eprintln!("{}", ui::info("Aborted; nothing was deleted."));
            return Ok(());
        }
        for resource in &resources {
            provider
                .delete_managed_resource_in(session.as_ref(), resource)
                .await?;
            eprintln!(
                "{}",
                ui::action(&format!("Deleted {} {}", resource.kind, resource.id))
            );
        }
        eprintln!(
            "{}",
            ui::success(&format!("Removed {} resource(s).", resources.len()))
        );
        Ok(())
    }

    async fn cleanup_roles_all(
        &self,
        provider: &dyn CloudProvider,
        config: &Config,
        dry_run: bool,
        assume_yes: bool,
    ) -> Result<()> {
        let post_login_actions = resolve_post_login_actions(&self.options, config);
        let session = provider
            .ensure_session(self.options.ignore_cache, post_login_actions)
            .await?;

        let spinner =
            ui::spinner("Scanning all reachable accounts for roleman-managed resources...");
        let accounts = provider
            .list_managed_resources_all(session.as_ref(), &|status: &str| {
                spinner.set_message(status.to_string());
            })
            .await?;
        spinner.finish_and_clear();

        let mut total = 0usize;
        for account in &accounts {
            if let Some(err) = &account.error {
                debug!(account = %account.account_id, error = %err, "skipped account during cleanup sweep");
                continue;
            }
            if account.resources.is_empty() {
                continue;
            }
            eprintln!("{} ({})", account.account_name, account.account_id);
            for resource in &account.resources {
                total += 1;
                eprintln!("  {} {} — {}", resource.kind, resource.id, resource.detail);
            }
        }
        let skipped = accounts.iter().filter(|a| a.error.is_some()).count();
        if skipped > 0 {
            eprintln!(
                "{}",
                ui::info(&format!(
                    "{skipped} account(s) skipped (no IAM access; roleman can't have created roles there)."
                ))
            );
        }
        if total == 0 {
            eprintln!(
                "{}",
                ui::info("No roleman-managed resources found in any account.")
            );
            return Ok(());
        }
        if dry_run {
            eprintln!("{}", ui::info("Dry run: nothing was deleted."));
            return Ok(());
        }
        if !assume_yes
            && !prompt_yes_no(&format!(
                "Delete {total} resource(s) across all accounts? [y/N] "
            ))?
        {
            eprintln!("{}", ui::info("Aborted; nothing was deleted."));
            return Ok(());
        }
        let mut removed = 0usize;
        for account in &accounts {
            for resource in &account.resources {
                match provider
                    .delete_managed_resource_in(session.as_ref(), resource)
                    .await
                {
                    Ok(()) => {
                        removed += 1;
                        eprintln!(
                            "{}",
                            ui::action(&format!(
                                "Deleted {} {} in {}",
                                resource.kind, resource.id, account.account_id
                            ))
                        );
                    }
                    Err(err) => eprintln!(
                        "{}",
                        ui::warn(&format!(
                            "Failed to delete {} {} in {}: {err}",
                            resource.kind, resource.id, account.account_id
                        ))
                    ),
                }
            }
        }
        eprintln!(
            "{}",
            ui::success(&format!("Removed {removed} of {total} resource(s)."))
        );
        Ok(())
    }

    pub async fn run(&self) -> Result<()> {
        let (mut config, config_path) = Config::load(self.options.config_path.as_deref())?;
        let config_exists = config_path.exists();
        let identity = resolve_identity(&self.options, &mut config, &config_path, config_exists)?;
        let provider = provider::for_identity(&identity)?;
        let post_login_actions = resolve_post_login_actions(&self.options, &config);
        let scope = self.options.scope;

        if matches!(self.options.action, AppAction::Login) {
            provider
                .ensure_session(self.options.ignore_cache, post_login_actions)
                .await?;
            return Ok(());
        }

        let context = self
            .prepare_visible_roles(provider.as_ref(), &identity)
            .await?;

        let prompt = match self.options.action {
            AppAction::Set => "roleman> ",
            AppAction::Open => "roleman open> ",
            AppAction::Login => unreachable!("login exits before role selection"),
            AppAction::List => unreachable!("list is handled by App::list_roles"),
        };
        let markers = provider.active_markers(&context.visible, scope);
        let selected = select_role_async(
            prompt,
            &context.visible,
            markers,
            self.options.initial_query.as_deref(),
        )
        .await?;
        if let Some(selection) = selected {
            if selection.auto_selected {
                eprintln!(
                    "{}",
                    ui::info(&format!("Using {}.", selection.choice.label()))
                );
            }
            let choice = selection.choice;
            tracing::debug!(
                account_id = %choice.account_id,
                account_name = %choice.account_name,
                role_name = %choice.role_name,
                "selected role"
            );
            if let Err(err) = history::record_selection(&identity.name, &choice) {
                debug!(error = %err, "failed to record history selection");
            }
            if matches!(self.options.action, AppAction::Set) && selection.open_in_browser {
                let url = provider.console_url(&choice);
                eprintln!("{}", ui::action(&format!("Opening {url}")));
                open_in_browser(&url)?;
                return Ok(());
            }
            match self.options.action {
                AppAction::Set => {
                    let namespace = provider.cache_namespace();
                    let cached = if self.options.ignore_cache {
                        None
                    } else if let Some(json) = credentials_cache::load_cached_payload(
                        &namespace,
                        &choice.account_id,
                        &choice.role_name,
                        scope,
                    )? {
                        Some(provider.credentials_from_cache_json(&json)?)
                    } else {
                        None
                    };
                    let creds = if let Some(creds) = cached {
                        tracing::debug!("using cached role credentials");
                        eprintln!("{}", ui::info("Using cached role credentials."));
                        creds
                    } else {
                        tracing::debug!("fetching role credentials");
                        let may_create = config.auto_create_readonly_roles.unwrap_or(false)
                            || self.options.assume_yes;
                        let fresh = fetch_with_consent(
                            provider.as_ref(),
                            context.session.as_ref(),
                            &choice,
                            scope,
                            may_create,
                        )
                        .await?;
                        credentials_cache::save_cached_payload(
                            &namespace,
                            &choice.account_id,
                            &choice.role_name,
                            scope,
                            fresh.expiration_ms(),
                            &fresh.to_cache_json()?,
                        )?;
                        tracing::debug!("role credentials received");
                        fresh
                    };
                    let omit_role_name =
                        has_single_role_for_account(&context.visible, &choice.account_id);
                    let binding = provider.ensure_profile(
                        context.session.as_ref(),
                        &choice,
                        scope,
                        omit_role_name,
                    )?;
                    let lines = provider::export_lines(&creds.env_vars(&binding));
                    if let Some(path) = env_file_path(&self.options) {
                        tracing::debug!(path = %path.display(), "writing env file");
                        write_env_file(&path, &lines)?;
                    }
                    let should_print =
                        self.options.print_env || env_file_path(&self.options).is_none();
                    if should_print {
                        println!("{}", lines);
                    }
                }
                AppAction::Open => {
                    let url = provider.console_url(&choice);
                    eprintln!("{}", ui::action(&format!("Opening {url}")));
                    open_in_browser(&url)?;
                }
                AppAction::Login => unreachable!("login exits before role selection"),
                AppAction::List => unreachable!("list is handled by App::list_roles"),
            }
        }

        Ok(())
    }

    async fn prepare_visible_roles(
        &self,
        provider: &dyn CloudProvider,
        identity: &SsoIdentity,
    ) -> Result<RoleSelectionContext> {
        let (config, _) = Config::load(self.options.config_path.as_deref())?;
        let refresh_seconds = self.options.refresh_seconds.or(config.refresh_seconds);
        let selector_sort = self.options.selector_sort.unwrap_or(config.selector_sort);
        let post_login_actions = resolve_post_login_actions(&self.options, &config);

        let (mut session, mut choices) =
            fetch_choices_with_cache(provider, self.options.ignore_cache, post_login_actions)
                .await?;

        apply_visible_role_preferences(
            &mut choices,
            identity,
            self.options.show_all,
            selector_sort,
            self.options.initial_query.as_deref(),
        );

        let mut visible = choices;
        if visible.is_empty()
            && let Some(seconds) = refresh_seconds
        {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
                let (refreshed_session, mut refreshed) = fetch_choices_with_cache(
                    provider,
                    self.options.ignore_cache,
                    post_login_actions,
                )
                .await?;
                session = refreshed_session;
                apply_visible_role_preferences(
                    &mut refreshed,
                    identity,
                    self.options.show_all,
                    selector_sort,
                    self.options.initial_query.as_deref(),
                );
                visible = refreshed;
                if !visible.is_empty() {
                    break;
                }
            }
        }

        Ok(RoleSelectionContext { session, visible })
    }
}

struct RoleSelectionContext {
    session: Box<dyn ProviderSession>,
    visible: Vec<RoleChoice>,
}

fn write_env_file(path: &PathBuf, lines: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| Error::Config(err.to_string()))?;
    }
    std::fs::write(path, lines)
        .map_err(|err| Error::Config(err.to_string()))
        .map(|_| {
            tracing::trace!(path = %path.display(), "wrote env file");
        })
}

fn open_in_browser(url: &str) -> Result<()> {
    open::that(url).map_err(|err| Error::OpenBrowser(err.to_string()))
}

fn env_file_path(options: &AppOptions) -> Option<PathBuf> {
    if let Some(path) = &options.env_file {
        tracing::debug!(path = %path.display(), "using env file from option");
        return Some(path.clone());
    }
    if let Ok(path) = std::env::var("_ROLEMAN_HOOK_ENV")
        && !path.is_empty()
    {
        let path = PathBuf::from(path);
        tracing::debug!(path = %path.display(), "using env file from _ROLEMAN_HOOK_ENV");
        return Some(path);
    }
    tracing::debug!("no env file path configured");
    None
}

async fn select_role_async(
    prompt: &str,
    choices: &[RoleChoice],
    markers: Vec<crate::provider::ActiveMarker>,
    initial_query: Option<&str>,
) -> Result<Option<tui::TuiSelection>> {
    let prompt = prompt.to_string();
    let choices = choices.to_vec();
    let initial_query = initial_query.map(ToOwned::to_owned);
    tokio::task::spawn_blocking(move || {
        tui::select_role(&prompt, &choices, &markers, initial_query.as_deref())
    })
    .await
    .map_err(|err| Error::Tui(format!("failed to join tui task: {err}")))?
}

/// Fetch credentials, obtaining consent if the provider needs to create a cloud resource.
///
/// `may_create` pre-authorizes creation (config knob or `--yes`). Otherwise, on
/// [`Error::NeedsResourceCreation`] we prompt interactively and retry; non-interactive runs
/// get an actionable error instead of a silent hang.
async fn fetch_with_consent(
    provider: &dyn CloudProvider,
    session: &dyn ProviderSession,
    choice: &RoleChoice,
    scope: AccessScope,
    may_create: bool,
) -> Result<Box<dyn ProviderCredentials>> {
    let spinner = ui::spinner("Fetching role credentials...");
    match provider
        .fetch_credentials(session, choice, scope, may_create)
        .await
    {
        Ok(creds) => {
            spinner.finish_with_message(ui::success("Fetched role credentials"));
            Ok(creds)
        }
        Err(Error::NeedsResourceCreation(desc)) => {
            spinner.finish_and_clear();
            if !std::io::stdin().is_terminal() {
                return Err(Error::NeedsResourceCreation(format!(
                    "{desc}. Re-run interactively, pass --yes, or set \
                     `auto_create_readonly_roles = true` in config."
                )));
            }
            eprintln!("{}", ui::warn(&format!("{desc}.")));
            if !prompt_yes_no("Create this role now? [y/N] ")? {
                return Err(Error::NeedsResourceCreation(
                    "declined to create the read-only role".to_string(),
                ));
            }
            let spinner = ui::spinner("Creating role and fetching credentials...");
            match provider
                .fetch_credentials(session, choice, scope, true)
                .await
            {
                Ok(creds) => {
                    spinner.finish_with_message(ui::success("Fetched role credentials"));
                    Ok(creds)
                }
                Err(err) => {
                    spinner.finish_and_clear();
                    Err(err)
                }
            }
        }
        Err(err) => {
            spinner.finish_and_clear();
            Err(err)
        }
    }
}

async fn fetch_choices_with_cache(
    provider: &dyn CloudProvider,
    ignore_cache: bool,
    post_login_actions: PostLoginActions,
) -> Result<(Box<dyn ProviderSession>, Vec<RoleChoice>)> {
    let session = provider
        .ensure_session(ignore_cache, post_login_actions)
        .await?;
    let namespace = provider.cache_namespace();

    if !ignore_cache && let Some((choices, age)) = roles_cache::load_cached_roles(&namespace)? {
        eprintln!(
            "{}",
            ui::info(&format!(
                "Using cached account/role list (updated {} ago).",
                roles_cache::format_age(age)
            ))
        );
        return Ok((session, choices));
    }

    let cached_fallback = if ignore_cache {
        None
    } else {
        roles_cache::load_cached_roles_with_age(&namespace)?
    };

    let choices = match provider.list_choices(session.as_ref()).await {
        Ok(choices) => choices,
        Err(err) => {
            if let Some((choices, age)) = cached_fallback {
                eprintln!(
                    "{}",
                    ui::warn(&format!(
                        "Failed to refresh account/role list; using cached data from {} ago.",
                        roles_cache::format_age(age)
                    ))
                );
                return Ok((session, choices));
            }
            return Err(err);
        }
    };

    roles_cache::save_cached_roles(&namespace, &choices)?;
    Ok((session, choices))
}

fn apply_visible_role_preferences(
    choices: &mut Vec<RoleChoice>,
    identity: &SsoIdentity,
    show_all: bool,
    selector_sort: SelectorSortMode,
    initial_query: Option<&str>,
) {
    if !show_all {
        apply_account_filters(choices, identity);
    }
    sort_choices(choices, identity);
    if matches!(selector_sort, SelectorSortMode::Dynamic)
        && let Err(err) = history::apply_history_sort(choices, &identity.name, initial_query)
    {
        debug!(error = %err, "failed to apply history sort");
    }
}

fn resolve_post_login_actions(options: &AppOptions, config: &Config) -> PostLoginActions {
    PostLoginActions {
        focus_terminal: options.focus_terminal_after_auth
            || config.focus_terminal_after_auth.unwrap_or(true),
        close_browser_tab: options.close_auth_tab || config.close_auth_tab.unwrap_or(false),
    }
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SelectorSortMode;
    use tempfile::TempDir;

    #[test]
    fn writes_env_file() {
        use crate::provider::{EnvVar, export_lines};

        let temp = TempDir::new().unwrap();
        let path = temp.path().join("env.sh");
        let vars = vec![
            EnvVar::new("AWS_ACCESS_KEY_ID", "AKIA123"),
            EnvVar::new("AWS_PROFILE", "Acme-Cloud/ReadOnly"),
        ];

        write_env_file(&path, &export_lines(&vars)).unwrap();
        let contents = std::fs::read_to_string(path).unwrap();
        assert!(contents.contains("AWS_ACCESS_KEY_ID=AKIA123"));
        assert!(contents.contains("AWS_PROFILE=Acme-Cloud/ReadOnly"));
    }

    #[test]
    fn guesses_account_name_from_url() {
        assert_eq!(guess_account_name("https://acme.awsapps.com/start"), "acme");
        assert_eq!(guess_account_name("https://my-org.awsapps.com/"), "my-org");
    }

    #[test]
    fn sorts_choices_by_precedence_then_name() {
        let identity = SsoIdentity {
            name: "acme".into(),
            start_url: "https://acme.awsapps.com/start".into(),
            sso_region: "us-east-1".into(),
            provider: config::ProviderKind::Aws,
            readonly_policy: None,
            accounts: vec![
                config::AccountRule {
                    account_id: "2222".into(),
                    alias: None,
                    ignored: false,
                    ignored_roles: Vec::new(),
                    precedence: Some(5),
                },
                config::AccountRule {
                    account_id: "1111".into(),
                    alias: None,
                    ignored: false,
                    ignored_roles: Vec::new(),
                    precedence: None,
                },
            ],
            ignore_roles: Vec::new(),
        };

        let mut choices = vec![
            RoleChoice {
                account_id: "1111".into(),
                account_name: "Zulu".into(),
                role_name: "ReadOnly".into(),
            },
            RoleChoice {
                account_id: "2222".into(),
                account_name: "Alpha".into(),
                role_name: "Admin".into(),
            },
            RoleChoice {
                account_id: "1111".into(),
                account_name: "Zulu".into(),
                role_name: "Admin".into(),
            },
        ];

        sort_choices(&mut choices, &identity);

        assert_eq!(choices[0].account_id, "2222");
        assert_eq!(choices[0].role_name, "Admin");
        assert_eq!(choices[1].role_name, "Admin");
        assert_eq!(choices[2].role_name, "ReadOnly");
    }

    #[test]
    fn detects_accounts_with_single_role() {
        let choices = vec![
            RoleChoice {
                account_id: "1111".into(),
                account_name: "Acme".into(),
                role_name: "ReadOnly".into(),
            },
            RoleChoice {
                account_id: "2222".into(),
                account_name: "Beta".into(),
                role_name: "Admin".into(),
            },
            RoleChoice {
                account_id: "2222".into(),
                account_name: "Beta".into(),
                role_name: "ReadOnly".into(),
            },
        ];

        assert!(has_single_role_for_account(&choices, "1111"));
        assert!(!has_single_role_for_account(&choices, "2222"));
        assert!(!has_single_role_for_account(&choices, "3333"));
    }

    #[test]
    fn resolves_post_login_actions_from_config_defaults() {
        let config = Config {
            identities: Vec::new(),
            default_identity: None,
            refresh_seconds: None,
            focus_terminal_after_auth: Some(true),
            close_auth_tab: Some(false),
            prompt_for_hook: None,
            hook_prompt: None,
            selector_sort: SelectorSortMode::Dynamic,
            auto_create_readonly_roles: None,
        };
        let options = AppOptions::default();

        let actions = resolve_post_login_actions(&options, &config);
        assert!(actions.focus_terminal);
        assert!(!actions.close_browser_tab);
    }

    #[test]
    fn resolves_post_login_actions_from_builtin_defaults() {
        let config = Config {
            identities: Vec::new(),
            default_identity: None,
            refresh_seconds: None,
            focus_terminal_after_auth: None,
            close_auth_tab: None,
            prompt_for_hook: None,
            hook_prompt: None,
            selector_sort: SelectorSortMode::Dynamic,
            auto_create_readonly_roles: None,
        };
        let options = AppOptions::default();

        let actions = resolve_post_login_actions(&options, &config);
        assert!(actions.focus_terminal);
        assert!(!actions.close_browser_tab);
    }

    #[test]
    fn cli_post_login_flags_override_config_defaults() {
        let config = Config {
            identities: Vec::new(),
            default_identity: None,
            refresh_seconds: None,
            focus_terminal_after_auth: Some(false),
            close_auth_tab: Some(false),
            prompt_for_hook: None,
            hook_prompt: None,
            selector_sort: SelectorSortMode::Dynamic,
            auto_create_readonly_roles: None,
        };
        let options = AppOptions {
            focus_terminal_after_auth: true,
            close_auth_tab: true,
            ..AppOptions::default()
        };

        let actions = resolve_post_login_actions(&options, &config);
        assert!(actions.focus_terminal);
        assert!(actions.close_browser_tab);
    }

    #[test]
    fn login_manual_identity_skips_config_save_prompt() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.toml");
        let mut config = Config::default();
        let options = AppOptions {
            start_url: Some("https://acme.awsapps.com/start".into()),
            sso_region: Some("us-east-1".into()),
            action: AppAction::Login,
            ..AppOptions::default()
        };

        let identity = resolve_identity(&options, &mut config, &config_path, false).unwrap();

        assert_eq!(identity.name, "manual");
        assert!(config.identities.is_empty());
        assert!(!config_path.exists());
    }

    #[test]
    fn list_manual_identity_skips_config_save_prompt() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.toml");
        let mut config = Config::default();
        let options = AppOptions {
            start_url: Some("https://acme.awsapps.com/start".into()),
            sso_region: Some("us-east-1".into()),
            action: AppAction::List,
            ..AppOptions::default()
        };

        let identity = resolve_identity(&options, &mut config, &config_path, false).unwrap();

        assert_eq!(identity.name, "manual");
        assert!(config.identities.is_empty());
        assert!(!config_path.exists());
    }
}

fn resolve_identity(
    options: &AppOptions,
    config: &mut Config,
    config_path: &Path,
    config_exists: bool,
) -> Result<SsoIdentity> {
    if let Some(name) = options.account.as_deref() {
        return config
            .identities
            .iter()
            .find(|identity| identity.name == name)
            .cloned()
            .ok_or(Error::MissingAccount);
    }

    if let Some(start_url) = options.start_url.clone() {
        let region = options.sso_region.clone().ok_or(Error::MissingRegion)?;
        let identity = SsoIdentity {
            name: "manual".to_string(),
            start_url,
            sso_region: region,
            provider: crate::config::ProviderKind::Aws,
            accounts: Vec::new(),
            ignore_roles: Vec::new(),
            readonly_policy: None,
        };
        if !matches!(options.action, AppAction::Login | AppAction::List)
            && !config_exists
            && config.identities.is_empty()
        {
            maybe_save_account(config, config_path, &identity)?;
        }
        return Ok(identity);
    }

    if let Some(default_name) = config.default_identity.as_deref()
        && let Some(identity) = config.identities.iter().find(|a| a.name == default_name)
    {
        return Ok(identity.clone());
    }
    if config.identities.len() == 1 {
        return Ok(config.identities[0].clone());
    }
    if config.identities.is_empty() {
        return Err(Error::MissingAccount);
    }

    prompt_select_account(&config.identities)
}

fn apply_account_filters(choices: &mut Vec<RoleChoice>, identity: &SsoIdentity) {
    if !identity.ignore_roles.is_empty() {
        choices.retain(|choice| !identity.ignore_roles.iter().any(|r| r == &choice.role_name));
    }
    if !identity.accounts.is_empty() {
        choices.retain_mut(|choice| {
            if let Some(rule) = identity
                .accounts
                .iter()
                .find(|rule| rule.account_id == choice.account_id)
            {
                if rule.ignored {
                    return false;
                }
                if let Some(alias) = &rule.alias
                    && !alias.trim().is_empty()
                {
                    choice.account_name = alias.clone();
                }
                if rule.ignored_roles.iter().any(|r| r == &choice.role_name) {
                    return false;
                }
            }
            true
        });
    }
}

fn sort_choices(choices: &mut [RoleChoice], identity: &SsoIdentity) {
    let mut precedence = std::collections::HashMap::new();
    for rule in &identity.accounts {
        if let Some(value) = rule.precedence {
            precedence.insert(rule.account_id.clone(), value);
        }
    }
    choices.sort_by_key(|choice| {
        let priority = precedence.get(&choice.account_id).copied().unwrap_or(0);
        (
            std::cmp::Reverse(priority),
            choice.account_name.to_lowercase(),
            choice.role_name.to_lowercase(),
        )
    });
}

fn has_single_role_for_account(choices: &[RoleChoice], account_id: &str) -> bool {
    choices
        .iter()
        .filter(|choice| choice.account_id == account_id)
        .take(2)
        .count()
        == 1
}

fn maybe_save_account(
    config: &mut Config,
    config_path: &Path,
    account: &SsoIdentity,
) -> Result<()> {
    if !prompt_yes_no("No config found. Save this SSO account as default? [y/N] ")? {
        return Ok(());
    }
    let suggested = guess_account_name(&account.start_url);
    let prompt = format!("Account name [{}]: ", suggested);
    let name = prompt_input(&prompt)?;
    let final_name = if name.trim().is_empty() {
        suggested
    } else {
        name.trim().to_string()
    };
    if final_name.is_empty() {
        return Ok(());
    }
    let account = SsoIdentity {
        name: final_name,
        start_url: account.start_url.clone(),
        sso_region: account.sso_region.clone(),
        provider: account.provider,
        accounts: Vec::new(),
        ignore_roles: Vec::new(),
        readonly_policy: account.readonly_policy.clone(),
    };
    config.default_identity = Some(account.name.clone());
    config.identities.push(account);
    config.save(config_path)?;
    Ok(())
}

fn prompt_select_account(accounts: &[SsoIdentity]) -> Result<SsoIdentity> {
    eprintln!("Select SSO account:");
    for (idx, account) in accounts.iter().enumerate() {
        eprintln!("  {}. {} ({})", idx + 1, account.name, account.sso_region);
    }
    let input = prompt_input("Enter choice: ")?;
    let index = input
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|v| v.checked_sub(1));
    if let Some(index) = index
        && let Some(account) = accounts.get(index)
    {
        return Ok(account.clone());
    }
    Err(Error::MissingAccount)
}

fn prompt_yes_no(prompt: &str) -> Result<bool> {
    let input = prompt_input(prompt)?;
    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn prompt_input(prompt: &str) -> Result<String> {
    use std::io::{self, Write};
    let mut stdout = io::stdout();
    stdout
        .write_all(prompt.as_bytes())
        .map_err(|err| Error::Config(err.to_string()))?;
    stdout
        .flush()
        .map_err(|err| Error::Config(err.to_string()))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|err| Error::Config(err.to_string()))?;
    Ok(input)
}

fn guess_account_name(start_url: &str) -> String {
    let host = start_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or_default();
    let subdomain = host.split('.').next().unwrap_or_default();
    let name = subdomain
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    name.trim_matches('-').to_string()
}
