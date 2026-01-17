mod aws_config;
mod aws_sdk;
mod config;
mod error;
mod model;
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

        let (mut cache, choices) =
            fetch_choices_with_cache(&start_url, sso_region.as_deref()).await?;

        if self.options.manage_hidden {
            let updated = tui::manage_hidden(&choices, &config.hidden_roles)?;
            config.hidden_roles = updated;
            config.save(&config_path)?;
            return Ok(());
        }

        let mut visible = filter_hidden(&choices, &config.hidden_roles);
        if visible.is_empty() {
            if let Some(seconds) = refresh_seconds {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
                    let (refreshed_cache, refreshed) =
                        fetch_choices_with_cache(&start_url, sso_region.as_deref()).await?;
                    cache = refreshed_cache;
                    visible = filter_hidden(&refreshed, &config.hidden_roles);
                    if !visible.is_empty() {
                        break;
                    }
                }
            }
        }

        let selected = tui::select_role(&visible)?;
        if let Some(choice) = selected {
            eprintln!("Fetching role credentials...");
            let profile_name = aws_config::profile_name_for(&choice);
            let creds = aws_sdk::get_role_credentials(
                &cache.access_token,
                &cache.region,
                &choice.account_id,
                &choice.role_name,
            )
            .await?;
            let env = EnvVars::from_role_credentials(&creds, &profile_name, &cache.region);
            println!("{}", env.to_export_lines());
        }

        Ok(())
    }
}

async fn fetch_choices_with_cache(
    start_url: &str,
    sso_region: Option<&str>,
) -> Result<(crate::model::CacheEntry, Vec<RoleChoice>)> {
    let cache = cache_token(start_url, sso_region).await?;
    let mut choices = Vec::new();
    eprintln!("Fetching SSO accounts...");
    let accounts = aws_sdk::list_accounts(&cache.access_token, &cache.region).await?;
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

    let roles_by_account = roles_by_account
        .into_iter()
        .collect::<Result<Vec<_>>>()?;

    for (account, roles) in roles_by_account {
        for role in roles {
            choices.push(RoleChoice::new(&account, &role));
        }
    }
    Ok((cache, choices))
}

async fn cache_token(
    start_url: &str,
    sso_region: Option<&str>,
) -> Result<crate::model::CacheEntry> {
    match sso_cache::load_valid_cache(start_url) {
        Ok(entry) => Ok(entry),
        Err(_) => {
            let region = sso_region.ok_or(Error::MissingRegion)?;
            sso_cache::device_authorization(start_url, region).await
        }
    }
}

fn filter_hidden(choices: &[RoleChoice], hidden: &[HiddenRole]) -> Vec<RoleChoice> {
    choices
        .iter()
        .cloned()
        .filter(|choice| !hidden.iter().any(|entry| entry.matches(choice)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Account, Role};

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
}
