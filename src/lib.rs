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
use crate::config::{Config, HiddenRole};
use std::path::PathBuf;
use futures::StreamExt;
use tracing::debug;

pub struct App {
    options: AppOptions,
}

#[derive(Debug, Default)]
pub struct AppOptions {
    pub start_url: Option<String>,
    pub sso_region: Option<String>,
    pub manage_hidden: bool,
    pub refresh_seconds: Option<u64>,
    pub config_path: Option<PathBuf>,
    pub ignore_cache: bool,
    pub env_file: Option<PathBuf>,
    pub print_env: bool,
}


impl App {
    pub fn new(options: AppOptions) -> Self {
        Self { options }
    }

    pub async fn run(&self) -> Result<()> {
        let (mut config, config_path) = Config::load(self.options.config_path.as_deref())?;
        let start_url = self
            .options
            .start_url
            .clone()
            .or_else(|| config.sso_start_url.clone())
            .ok_or(Error::MissingStartUrl)?;
        let sso_region = self.options.sso_region.clone().or(config.sso_region.clone());
        let refresh_seconds = self.options.refresh_seconds.or(config.refresh_seconds);

        let (mut cache, choices) = fetch_choices_with_cache(
            &start_url,
            sso_region.as_deref(),
            self.options.ignore_cache,
        )
        .await?;

        if self.options.manage_hidden {
            let updated = tui::manage_hidden(&choices, &config.hidden_roles)?;
            config.hidden_roles = updated;
            config.save(&config_path)?;
            return Ok(());
        }

        let mut visible = filter_hidden(&choices, &config.hidden_roles);
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
                visible = filter_hidden(&refreshed, &config.hidden_roles);
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

fn filter_hidden(choices: &[RoleChoice], hidden: &[HiddenRole]) -> Vec<RoleChoice> {
    choices
        .iter()
        .filter(|choice| !hidden.iter().any(|entry| entry.matches(choice)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Account, Role};
    use tempfile::TempDir;

    #[test]
    fn filters_hidden_roles() {
        let account = Account {
            id: "1234".into(),
            name: "Main".into(),
        };
        let admin = Role { name: "Admin".into() };
        let read_only = Role { name: "ReadOnly".into() };

        let choices = vec![RoleChoice::new(&account, &admin), RoleChoice::new(&account, &read_only)];
        let hidden = vec![HiddenRole {
            account_id: "1234".into(),
            role_name: "Admin".into(),
        }];

        let visible = filter_hidden(&choices, &hidden);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].role_name, "ReadOnly");
    }

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
