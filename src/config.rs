use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::RoleChoice;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub sso_start_url: Option<String>,
    pub sso_region: Option<String>,
    pub refresh_seconds: Option<u64>,
    #[serde(default)]
    pub hidden_roles: Vec<HiddenRole>,
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
        let contents = toml::to_string_pretty(self).map_err(|err| Error::Config(err.to_string()))?;
        fs::write(path, contents).map_err(|err| Error::Config(err.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct HiddenRole {
    pub account_id: String,
    pub role_name: String,
}

impl HiddenRole {
    pub fn from_choice(choice: &RoleChoice) -> Self {
        Self {
            account_id: choice.account_id.clone(),
            role_name: choice.role_name.clone(),
        }
    }

    pub fn matches(&self, choice: &RoleChoice) -> bool {
        self.account_id == choice.account_id && self.role_name == choice.role_name
    }
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
            sso_start_url: Some("https://example.awsapps.com/start".into()),
            sso_region: Some("us-east-1".into()),
            refresh_seconds: Some(120),
            hidden_roles: vec![HiddenRole {
                account_id: "1234".into(),
                role_name: "Admin".into(),
            }],
        };

        config.save(&path).unwrap();
        let (loaded, _) = Config::load(Some(&path)).unwrap();
        assert_eq!(loaded.sso_start_url, config.sso_start_url);
        assert_eq!(loaded.sso_region, config.sso_region);
        assert_eq!(loaded.refresh_seconds, config.refresh_seconds);
        assert_eq!(loaded.hidden_roles, config.hidden_roles);
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
        assert!(config.sso_start_url.is_none());
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
