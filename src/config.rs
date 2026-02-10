use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub identities: Vec<SsoIdentity>,
    pub default_identity: Option<String>,
    pub refresh_seconds: Option<u64>,
    pub focus_terminal_after_auth: Option<bool>,
    pub close_auth_tab: Option<bool>,
    pub prompt_for_hook: Option<bool>,
    pub hook_prompt: Option<HookPromptMode>,
    #[serde(default)]
    pub selector_sort: SelectorSortMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HookPromptMode {
    Always,
    Never,
    Outdated,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SelectorSortMode {
    #[default]
    Dynamic,
    Alphabetical,
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<(Self, PathBuf)> {
        let path = match path {
            Some(path) => path.to_path_buf(),
            None => default_config_path()?,
        };

        if !path.exists() {
            return Ok((Config::default(), path));
        }

        let contents = fs::read_to_string(&path).map_err(|err| Error::Config(err.to_string()))?;
        let config = toml::from_str(&contents).map_err(|err| Error::Config(err.to_string()))?;
        Ok((config, path))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| Error::Config(err.to_string()))?;
        }
        let contents =
            toml::to_string_pretty(self).map_err(|err| Error::Config(err.to_string()))?;
        fs::write(path, contents).map_err(|err| Error::Config(err.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SsoIdentity {
    pub name: String,
    pub start_url: String,
    pub sso_region: String,
    #[serde(default)]
    pub accounts: Vec<AccountRule>,
    #[serde(default)]
    pub ignore_roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountRule {
    pub account_id: String,
    pub alias: Option<String>,
    #[serde(default)]
    pub ignored: bool,
    #[serde(default)]
    pub ignored_roles: Vec<String>,
    #[serde(default)]
    pub precedence: Option<i32>,
}

fn default_config_path() -> Result<PathBuf> {
    let base = if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(dir)
    } else {
        let home = std::env::var("HOME").map_err(|_| Error::MissingHome)?;
        PathBuf::from(home).join(".config")
    };
    Ok(base.join("roleman").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn roundtrip_config() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        let config = Config {
            identities: vec![SsoIdentity {
                name: "work".into(),
                start_url: "https://example.awsapps.com/start".into(),
                sso_region: "us-east-1".into(),
                accounts: vec![AccountRule {
                    account_id: "1234".into(),
                    alias: Some("Main".into()),
                    ignored: false,
                    ignored_roles: vec!["Admin".into()],
                    precedence: Some(10),
                }],
                ignore_roles: vec!["ReadOnly".into()],
            }],
            default_identity: Some("work".into()),
            refresh_seconds: Some(120),
            focus_terminal_after_auth: Some(true),
            close_auth_tab: Some(false),
            prompt_for_hook: None,
            hook_prompt: None,
            selector_sort: SelectorSortMode::Alphabetical,
        };

        config.save(&path).unwrap();
        let (loaded, _) = Config::load(Some(&path)).unwrap();
        assert_eq!(loaded.identities, config.identities);
        assert_eq!(loaded.default_identity, config.default_identity);
        assert_eq!(loaded.refresh_seconds, config.refresh_seconds);
        assert_eq!(
            loaded.focus_terminal_after_auth,
            config.focus_terminal_after_auth
        );
        assert_eq!(loaded.close_auth_tab, config.close_auth_tab);
        assert_eq!(loaded.selector_sort, config.selector_sort);
    }

    #[test]
    fn default_path_uses_xdg_config_home() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_CONFIG_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", temp.path());
        }

        let (config, path) = Config::load(None).unwrap();
        assert!(config.identities.is_empty());
        assert_eq!(config.selector_sort, SelectorSortMode::Dynamic);
        assert_eq!(config.focus_terminal_after_auth, None);
        assert_eq!(config.close_auth_tab, None);
        assert_eq!(path, temp.path().join("roleman").join("config.toml"));

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_CONFIG_HOME", value);
            } else {
                std::env::remove_var("XDG_CONFIG_HOME");
            }
        }
    }
}
