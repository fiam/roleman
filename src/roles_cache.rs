use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::error::{Error, Result};
use crate::model::RoleChoice;

const ROLES_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Serialize, Deserialize)]
struct CachedRoles {
    fetched_at: u64,
    roles: Vec<CachedRole>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedRole {
    account_id: String,
    account_name: String,
    role_name: String,
}

pub fn load_cached_roles(start_url: &str) -> Result<Option<(Vec<RoleChoice>, Duration)>> {
    let cached = load_cached_roles_with_age(start_url)?;
    if let Some((choices, age)) = cached
        && age <= ROLES_CACHE_TTL
    {
        return Ok(Some((choices, age)));
    }
    Ok(None)
}

pub fn load_cached_roles_with_age(
    start_url: &str,
) -> Result<Option<(Vec<RoleChoice>, Duration)>> {
    let cache_dir = roleman_cache_dir()?;
    let path = cache_dir.join(cache_filename(start_url));
    if !path.exists() {
        return Ok(None);
    }

    let data = match fs::read_to_string(&path) {
        Ok(data) => data,
        Err(_) => return Ok(None),
    };
    let cached: CachedRoles = match serde_json::from_str(&data) {
        Ok(cached) => cached,
        Err(_) => return Ok(None),
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let age = Duration::from_secs(now.saturating_sub(cached.fetched_at));
    let choices = cached
        .roles
        .into_iter()
        .map(|entry| RoleChoice {
            account_id: entry.account_id,
            account_name: entry.account_name,
            role_name: entry.role_name,
        })
        .collect();
    Ok(Some((choices, age)))
}

pub fn save_cached_roles(start_url: &str, choices: &[RoleChoice]) -> Result<()> {
    let cache_dir = roleman_cache_dir()?;
    fs::create_dir_all(&cache_dir).map_err(|_| Error::MissingCache)?;
    let path = cache_dir.join(cache_filename(start_url));
    let cached = CachedRoles {
        fetched_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        roles: choices
            .iter()
            .map(|choice| CachedRole {
                account_id: choice.account_id.clone(),
                account_name: choice.account_name.clone(),
                role_name: choice.role_name.clone(),
            })
            .collect(),
    };
    let data =
        serde_json::to_string(&cached).map_err(|_| Error::CacheParse { path: path.clone() })?;
    fs::write(&path, data).map_err(|_| Error::CacheParse { path })?;
    Ok(())
}

pub fn format_age(age: Duration) -> String {
    let total = age.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn roleman_cache_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        Ok(PathBuf::from(dir).join("roleman"))
    } else {
        let home = std::env::var("HOME").map_err(|_| Error::MissingHome)?;
        Ok(Path::new(&home).join(".cache").join("roleman"))
    }
}

fn cache_filename(start_url: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(start_url.as_bytes());
    let digest = hasher.finalize();
    format!("roles-{:x}.json", digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn caches_roles_roundtrip() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_CACHE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", temp.path());
        }

        let choices = vec![
            RoleChoice {
                account_id: "1234".into(),
                account_name: "Main".into(),
                role_name: "Admin".into(),
            },
            RoleChoice {
                account_id: "1234".into(),
                account_name: "Main".into(),
                role_name: "ReadOnly".into(),
            },
        ];

        save_cached_roles("https://example.awsapps.com/start", &choices).unwrap();
        let loaded = load_cached_roles_with_age("https://example.awsapps.com/start").unwrap();
        assert!(loaded.is_some());
        let (roles, _age) = loaded.unwrap();
        assert_eq!(roles.len(), 2);

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_CACHE_HOME", value);
            } else {
                std::env::remove_var("XDG_CACHE_HOME");
            }
        }
    }

    #[test]
    fn stale_cache_is_ignored_by_default_loader() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_CACHE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", temp.path());
        }

        let cache_dir = roleman_cache_dir().unwrap();
        fs::create_dir_all(&cache_dir).unwrap();
        let path = cache_dir.join(cache_filename("https://example.awsapps.com/start"));
        let stale = CachedRoles {
            fetched_at: SystemTime::now()
                .checked_sub(ROLES_CACHE_TTL + Duration::from_secs(60))
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            roles: vec![CachedRole {
                account_id: "1234".into(),
                account_name: "Main".into(),
                role_name: "Admin".into(),
            }],
        };
        let data = serde_json::to_string(&stale).unwrap();
        fs::write(&path, data).unwrap();

        let fresh = load_cached_roles("https://example.awsapps.com/start").unwrap();
        assert!(fresh.is_none());
        let with_age = load_cached_roles_with_age("https://example.awsapps.com/start").unwrap();
        assert!(with_age.is_some());

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_CACHE_HOME", value);
            } else {
                std::env::remove_var("XDG_CACHE_HOME");
            }
        }
    }

    #[test]
    fn format_age_outputs_compact_string() {
        assert_eq!(format_age(Duration::from_secs(5)), "5s");
        assert_eq!(format_age(Duration::from_secs(70)), "1m 10s");
        assert_eq!(format_age(Duration::from_secs(3_650)), "1h 0m");
    }
}
