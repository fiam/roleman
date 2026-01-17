mod aws_config;
mod aws_sdk;
mod config;
mod error;
mod model;
mod roles_cache;
mod sso_cache;
mod tui;

pub use crate::error::{Error, Result};
use crate::model::{EnvVars, RoleChoice};
use crate::config::{Config, SsoAccount};
use std::path::{Path, PathBuf};
use futures::StreamExt;
use tracing::debug;

pub struct App {
    options: AppOptions,
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
}


impl App {
    pub fn new(options: AppOptions) -> Self {
        Self { options }
    }

    pub async fn run(&self) -> Result<()> {
        let (mut config, config_path) = Config::load(self.options.config_path.as_deref())?;
        let config_exists = config_path.exists();
        let account = resolve_account(
            &self.options,
            &mut config,
            &config_path,
            config_exists,
        )?;
        let start_url = account.start_url;
        let sso_region = Some(account.sso_region);
        let refresh_seconds = self.options.refresh_seconds.or(config.refresh_seconds);

        let (mut cache, choices) = fetch_choices_with_cache(
            &start_url,
            sso_region.as_deref(),
            self.options.ignore_cache,
        )
        .await?;

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
                if !visible.is_empty() {
                    break;
                }
            }
        }

        let selected = tui::select_role(&visible)?;
        if let Some(choice) = selected {
            tracing::debug!(
                account_id = %choice.account_id,
                account_name = %choice.account_name,
                role_name = %choice.role_name,
                "selected role"
            );
            tracing::debug!("fetching role credentials");
            eprintln!("Fetching role credentials...");
            let profile_name = aws_config::profile_name_for(&choice);
            let config_path = aws_config::ensure_profile_region(&profile_name, &cache.region)?;
            let creds = aws_sdk::get_role_credentials(
                &cache.access_token,
                &cache.region,
                &choice.account_id,
                &choice.role_name,
            )
            .await?;
            tracing::debug!("role credentials received");
            let mut env = EnvVars::from_role_credentials(&creds, &profile_name, &cache.region);
            env.config_file = Some(config_path.display().to_string());
            if let Some(path) = env_file_path(&self.options) {
                tracing::debug!(path = %path.display(), "writing env file");
                write_env_file(&path, &env)?;
            }
            if self.options.print_env {
            println!("{}", env.to_export_lines());
            }
        }

        Ok(())
    }
}

fn write_env_file(path: &PathBuf, env: &EnvVars) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| Error::Config(err.to_string()))?;
    }
    std::fs::write(path, env.to_export_lines()).map_err(|err| Error::Config(err.to_string()))
        .map(|_| {
            tracing::trace!(path = %path.display(), "wrote env file");
        })
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
    if !ignore_cache
        && let Some((choices, age)) = roles_cache::load_cached_roles(start_url)?
    {
        eprintln!(
            "Using cached account/role list (updated {} ago).",
            roles_cache::format_age(age)
        );
        return Ok((cache, choices));
    }
    if !ignore_cache
        && let Some((choices, age)) = roles_cache::load_cached_roles_with_age(start_url)?
    {
        cached_fallback = Some((choices, age));
    }

    let mut choices = Vec::new();
    eprintln!("Fetching SSO accounts...");
    let accounts = match aws_sdk::list_accounts(&cache.access_token, &cache.region).await {
        Ok(accounts) => accounts,
        Err(err) => {
            if let Some((choices, age)) = cached_fallback {
                eprintln!(
                    "Failed to refresh account/role list; using cached data from {} ago.",
                    roles_cache::format_age(age)
                );
                return Ok((cache, choices));
            }
            return Err(err);
        }
    };
    for account in &accounts {
        debug!(account_id = %account.id, account_name = %account.name, "fetched account");
    }

    eprintln!("Fetching roles for all accounts...");
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
                eprintln!(
                    "Failed to refresh account/role list; using cached data from {} ago.",
                    roles_cache::format_age(age)
                );
                return Ok((cache, choices));
            }
            return Err(err);
        }
    };

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
    if !ignore_cache
        && let Ok(entry) = sso_cache::load_valid_cache(start_url)
    {
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
            profile_name: "Docker-Cloud/ReadOnly".into(),
            config_file: None,
        };

        write_env_file(&path, &env).unwrap();
        let contents = std::fs::read_to_string(path).unwrap();
        assert!(contents.contains("AWS_ACCESS_KEY_ID=AKIA123"));
        assert!(contents.contains("AWS_PROFILE=Docker-Cloud/ReadOnly"));
    }
}

fn resolve_account(
    options: &AppOptions,
    config: &mut Config,
    config_path: &Path,
    config_exists: bool,
) -> Result<SsoAccount> {
    if let Some(name) = options.account.as_deref() {
        return config
            .accounts
            .iter()
            .find(|account| account.name == name)
            .cloned()
            .ok_or(Error::MissingAccount);
    }

    if let Some(start_url) = options.start_url.clone() {
        let region = options.sso_region.clone().ok_or(Error::MissingRegion)?;
        let account = SsoAccount {
            name: "manual".to_string(),
            start_url,
            sso_region: region,
        };
        if !config_exists && config.accounts.is_empty() {
            maybe_save_account(config, config_path, &account)?;
        }
        return Ok(account);
    }

    if let Some(default_name) = config.default_account.as_deref()
        && let Some(account) = config.accounts.iter().find(|a| a.name == default_name)
    {
        return Ok(account.clone());
    }
    if config.accounts.len() == 1 {
        return Ok(config.accounts[0].clone());
    }
    if config.accounts.is_empty() {
        return Err(Error::MissingAccount);
    }

    prompt_select_account(&config.accounts)
}

fn maybe_save_account(
    config: &mut Config,
    config_path: &Path,
    account: &SsoAccount,
) -> Result<()> {
    if !prompt_yes_no("No config found. Save this SSO account as default? [y/N] ")? {
        return Ok(());
    }
    let name = prompt_input("Account name: ")?;
    if name.trim().is_empty() {
        return Ok(());
    }
    let account = SsoAccount {
        name: name.trim().to_string(),
        start_url: account.start_url.clone(),
        sso_region: account.sso_region.clone(),
    };
    config.default_account = Some(account.name.clone());
    config.accounts.push(account);
    config.save(config_path)?;
    Ok(())
}

fn prompt_select_account(accounts: &[SsoAccount]) -> Result<SsoAccount> {
    eprintln!("Select SSO account:");
    for (idx, account) in accounts.iter().enumerate() {
        eprintln!(
            "  {}. {} ({})",
            idx + 1,
            account.name,
            account.sso_region
        );
    }
    let input = prompt_input("Enter choice: ")?;
    let index = input.trim().parse::<usize>().ok().and_then(|v| v.checked_sub(1));
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
    stdout.write_all(prompt.as_bytes()).map_err(|err| Error::Config(err.to_string()))?;
    stdout.flush().map_err(|err| Error::Config(err.to_string()))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|err| Error::Config(err.to_string()))?;
    Ok(input)
}
