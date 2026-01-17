mod aws_cli;
mod aws_config;
mod config;
mod error;
mod model;
mod sso_cache;
mod tui;

pub use crate::error::{Error, Result};
use crate::model::{EnvVars, RoleChoice};
use crate::config::{Config, HiddenRole};
use std::path::PathBuf;

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

    pub fn run(&self) -> Result<()> {
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
            fetch_choices_with_cache(&start_url, sso_region.as_deref())?;

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
                    std::thread::sleep(std::time::Duration::from_secs(seconds));
                    let (refreshed_cache, refreshed) =
                        fetch_choices_with_cache(&start_url, sso_region.as_deref())?;
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
            let profile_name = aws_config::profile_name_for(&choice);
            let creds = aws_cli::get_role_credentials(
                &cache.access_token,
                &cache.region,
                &choice.account_id,
                &choice.role_name,
            )?;
            let env = EnvVars::from_role_credentials(&creds, &profile_name, &cache.region);
            println!("{}", env.to_export_lines());
        }

        Ok(())
    }
}

fn fetch_choices_with_cache(
    start_url: &str,
    sso_region: Option<&str>,
) -> Result<(crate::model::CacheEntry, Vec<RoleChoice>)> {
    let cache = cache_token(start_url, sso_region)?;
    let mut choices = Vec::new();
    let accounts = aws_cli::list_accounts(&cache.access_token, &cache.region)?;
    for account in accounts {
        let roles = aws_cli::list_account_roles(&cache.access_token, &cache.region, &account.id)?;
        for role in roles {
            choices.push(RoleChoice::new(&account, &role));
        }
    }
    Ok((cache, choices))
}

fn cache_token(start_url: &str, sso_region: Option<&str>) -> Result<crate::model::CacheEntry> {
    sso_cache::load_valid_cache(start_url).or_else(|_| {
        let region = sso_region.ok_or(Error::MissingRegion)?;
        sso_cache::device_authorization(start_url, region)
    })
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
