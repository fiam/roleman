mod aws_config;
pub mod aws_sdk;
pub mod config;
mod credentials_cache;
mod error;
mod mock_server;
mod model;
mod roles_cache;
mod sso_cache;
mod tui;
pub mod ui;

pub use crate::config::Config;
use crate::config::SsoIdentity;
pub use crate::error::{Error, Result};
pub use crate::mock_server::{
    MockServerHandle, MockServerOptions, run_mock_server, start_mock_server,
};
use crate::model::{EnvVars, RoleChoice};
use futures::StreamExt;
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
    pub account: Option<String>,
    pub show_all: bool,
    pub action: AppAction,
}

impl App {
    pub fn new(options: AppOptions) -> Self {
        Self { options }
    }

    pub async fn run(&self) -> Result<()> {
        let (mut config, config_path) = Config::load(self.options.config_path.as_deref())?;
        let config_exists = config_path.exists();
        let identity = resolve_identity(&self.options, &mut config, &config_path, config_exists)?;
        let start_url = identity.start_url.clone();
        let sso_region = Some(identity.sso_region.clone());
        let refresh_seconds = self.options.refresh_seconds.or(config.refresh_seconds);

        let (mut cache, mut choices) =
            fetch_choices_with_cache(&start_url, sso_region.as_deref(), self.options.ignore_cache)
                .await?;

        if !self.options.show_all {
            apply_account_filters(&mut choices, &identity);
        }
        sort_choices(&mut choices, &identity);

        let mut visible = choices;
        if visible.is_empty()
            && let Some(seconds) = refresh_seconds
        {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
                let (refreshed_cache, refreshed) = fetch_choices_with_cache(
                    &start_url,
                    sso_region.as_deref(),
                    self.options.ignore_cache,
                )
                .await?;
                cache = refreshed_cache;
                visible = refreshed;
                if !self.options.show_all {
                    apply_account_filters(&mut visible, &identity);
                }
                sort_choices(&mut visible, &identity);
                if !visible.is_empty() {
                    break;
                }
            }
        }

        let prompt = match self.options.action {
            AppAction::Set => "roleman> ",
            AppAction::Open => "roleman open> ",
        };
        let selected = tui::select_role(prompt, &visible, &start_url, &cache.region)?;
        if let Some(selection) = selected {
            let choice = selection.choice;
            tracing::debug!(
                account_id = %choice.account_id,
                account_name = %choice.account_name,
                role_name = %choice.role_name,
                "selected role"
            );
            if matches!(self.options.action, AppAction::Set) && selection.open_in_browser {
                let url = console_url(&start_url, &choice.account_id, &choice.role_name);
                eprintln!("{}", ui::action(&format!("Opening {url}")));
                open_in_browser(&url)?;
                return Ok(());
            }
            match self.options.action {
                AppAction::Set => {
                    let cached_credentials = if self.options.ignore_cache {
                        None
                    } else {
                        credentials_cache::load_cached_credentials(
                            &start_url,
                            &cache.region,
                            &choice.account_id,
                            &choice.role_name,
                        )?
                    };
                    let creds = if let Some(creds) = cached_credentials {
                        tracing::debug!("using cached role credentials");
                        eprintln!("{}", ui::info("Using cached role credentials."));
                        creds
                    } else {
                        tracing::debug!("fetching role credentials");
                        let spinner = ui::spinner("Fetching role credentials...");
                        let fresh = aws_sdk::get_role_credentials(
                            &cache.access_token,
                            &cache.region,
                            &choice.account_id,
                            &choice.role_name,
                        )
                        .await?;
                        spinner.finish_with_message(ui::success("Fetched role credentials"));
                        credentials_cache::save_cached_credentials(
                            &start_url,
                            &cache.region,
                            &choice.account_id,
                            &choice.role_name,
                            &fresh,
                        )?;
                        tracing::debug!("role credentials received");
                        fresh
                    };
                    let profile_name = aws_config::profile_name_for(&choice);
                    let config_path =
                        aws_config::ensure_profile_region(&profile_name, &cache.region)?;
                    let mut env =
                        EnvVars::from_role_credentials(&creds, &profile_name, &cache.region);
                    env.config_file = Some(config_path.display().to_string());
                    if let Some(path) = env_file_path(&self.options) {
                        tracing::debug!(path = %path.display(), "writing env file");
                        write_env_file(&path, &env)?;
                    }
                    let should_print =
                        self.options.print_env || env_file_path(&self.options).is_none();
                    if should_print {
                        println!("{}", env.to_export_lines());
                    }
                }
                AppAction::Open => {
                    let url = console_url(&start_url, &choice.account_id, &choice.role_name);
                    eprintln!("{}", ui::action(&format!("Opening {url}")));
                    open_in_browser(&url)?;
                }
            }
        }

        Ok(())
    }
}

fn write_env_file(path: &PathBuf, env: &EnvVars) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| Error::Config(err.to_string()))?;
    }
    std::fs::write(path, env.to_export_lines())
        .map_err(|err| Error::Config(err.to_string()))
        .map(|_| {
            tracing::trace!(path = %path.display(), "wrote env file");
        })
}

fn console_url(start_url: &str, account_id: &str, role_name: &str) -> String {
    let base = start_url.trim_end_matches('/');
    format!(
        "{base}/#/console?account_id={account_id}&role_name={role}",
        role = urlencoding::encode(role_name)
    )
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

async fn fetch_choices_with_cache(
    start_url: &str,
    sso_region: Option<&str>,
    ignore_cache: bool,
) -> Result<(crate::model::CacheEntry, Vec<RoleChoice>)> {
    let cache = cache_token(start_url, sso_region, ignore_cache).await?;
    let mut cached_fallback: Option<(Vec<RoleChoice>, std::time::Duration)> = None;
    if !ignore_cache && let Some((choices, age)) = roles_cache::load_cached_roles(start_url)? {
        eprintln!(
            "{}",
            ui::info(&format!(
                "Using cached account/role list (updated {} ago).",
                roles_cache::format_age(age)
            ))
        );
        return Ok((cache, choices));
    }
    if !ignore_cache
        && let Some((choices, age)) = roles_cache::load_cached_roles_with_age(start_url)?
    {
        cached_fallback = Some((choices, age));
    }

    let mut choices = Vec::new();
    let accounts_spinner = ui::spinner("Fetching SSO accounts...");
    let mut accounts = match aws_sdk::list_accounts(&cache.access_token, &cache.region).await {
        Ok(accounts) => accounts,
        Err(err) => {
            if let Some((choices, age)) = cached_fallback {
                accounts_spinner.finish_and_clear();
                eprintln!(
                    "{}",
                    ui::warn(&format!(
                        "Failed to refresh account/role list; using cached data from {} ago.",
                        roles_cache::format_age(age)
                    ))
                );
                return Ok((cache, choices));
            }
            accounts_spinner.finish_and_clear();
            return Err(err);
        }
    };
    accounts_spinner.finish_with_message(ui::success("Fetched SSO accounts"));
    accounts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    for account in &accounts {
        debug!(account_id = %account.id, account_name = %account.name, "fetched account");
    }

    let roles_spinner = ui::spinner("Fetching roles for all accounts...");
    let roles_by_account = futures::stream::iter(accounts.clone())
        .map(|account| {
            let token = cache.access_token.clone();
            let region = cache.region.clone();
            async move {
                let roles = aws_sdk::list_account_roles(&token, &region, &account.id).await?;
                Ok::<_, Error>((account, roles))
            }
        })
        .buffer_unordered(10)
        .collect::<Vec<_>>()
        .await;

    let roles_by_account = match roles_by_account.into_iter().collect::<Result<Vec<_>>>() {
        Ok(roles) => roles,
        Err(err) => {
            if let Some((choices, age)) = cached_fallback {
                roles_spinner.finish_and_clear();
                eprintln!(
                    "{}",
                    ui::warn(&format!(
                        "Failed to refresh account/role list; using cached data from {} ago.",
                        roles_cache::format_age(age)
                    ))
                );
                return Ok((cache, choices));
            }
            roles_spinner.finish_and_clear();
            return Err(err);
        }
    };
    roles_spinner.finish_with_message(ui::success("Fetched roles"));

    for (account, roles) in roles_by_account {
        for role in roles {
            choices.push(RoleChoice::new(&account, &role));
        }
    }

    roles_cache::save_cached_roles(start_url, &choices)?;
    Ok((cache, choices))
}

async fn cache_token(
    start_url: &str,
    sso_region: Option<&str>,
    ignore_cache: bool,
) -> Result<crate::model::CacheEntry> {
    if !ignore_cache && let Ok(entry) = sso_cache::load_valid_cache(start_url) {
        return Ok(entry);
    }
    let region = sso_region.ok_or(Error::MissingRegion)?;
    sso_cache::device_authorization(start_url, region).await
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn writes_env_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("env.sh");
        let env = EnvVars {
            access_key_id: "AKIA123".into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            expiration_ms: 1_700_000_000_000,
            region: "us-east-1".into(),
            profile_name: "Acme-Cloud/ReadOnly".into(),
            config_file: None,
        };

        write_env_file(&path, &env).unwrap();
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
    fn builds_console_url() {
        let url = console_url(
            "https://acme.awsapps.com/start/",
            "123456789012",
            "Read Only",
        );
        assert_eq!(
            url,
            "https://acme.awsapps.com/start/#/console?account_id=123456789012&role_name=Read%20Only"
        );
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
            accounts: Vec::new(),
            ignore_roles: Vec::new(),
        };
        if !config_exists && config.identities.is_empty() {
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
        accounts: Vec::new(),
        ignore_roles: Vec::new(),
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
