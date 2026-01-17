use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::error::{Error, Result};
use crate::model::AwsRoleCredentials;
use crate::roles_cache::roleman_cache_dir;

const EXPIRY_SAFETY_SECS: u64 = 60;

#[derive(Debug, Serialize, Deserialize)]
struct CachedCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
    expiration_ms: u64,
}

pub fn load_cached_credentials(
    start_url: &str,
    region: &str,
    account_id: &str,
    role_name: &str,
) -> Result<Option<AwsRoleCredentials>> {
    let path = cache_path(start_url, region, account_id, role_name)?;
    if !path.exists() {
        return Ok(None);
    }
    let data = match fs::read_to_string(&path) {
        Ok(data) => data,
        Err(_) => return Ok(None),
    };
    let cached: CachedCredentials = match serde_json::from_str(&data) {
        Ok(cached) => cached,
        Err(_) => return Ok(None),
    };
    if is_expired(cached.expiration_ms)? {
        return Ok(None);
    }
    Ok(Some(AwsRoleCredentials {
        access_key_id: cached.access_key_id,
        secret_access_key: cached.secret_access_key,
        session_token: cached.session_token,
        expiration: cached.expiration_ms,
    }))
}

pub fn save_cached_credentials(
    start_url: &str,
    region: &str,
    account_id: &str,
    role_name: &str,
    creds: &AwsRoleCredentials,
) -> Result<()> {
    let path = cache_path(start_url, region, account_id, role_name)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| Error::MissingCache)?;
    }
    let cached = CachedCredentials {
        access_key_id: creds.access_key_id.clone(),
        secret_access_key: creds.secret_access_key.clone(),
        session_token: creds.session_token.clone(),
        expiration_ms: creds.expiration,
    };
    let data =
        serde_json::to_string(&cached).map_err(|_| Error::CacheParse { path: path.clone() })?;
    fs::write(&path, data).map_err(|_| Error::CacheParse { path })?;
    Ok(())
}

fn cache_path(start_url: &str, region: &str, account_id: &str, role_name: &str) -> Result<PathBuf> {
    let cache_dir = roleman_cache_dir()?;
    Ok(cache_dir.join(cache_filename(start_url, region, account_id, role_name)))
}

fn cache_filename(start_url: &str, region: &str, account_id: &str, role_name: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(start_url.as_bytes());
    hasher.update(region.as_bytes());
    hasher.update(account_id.as_bytes());
    hasher.update(role_name.as_bytes());
    let digest = hasher.finalize();
    format!("creds-{:x}.json", digest)
}

fn is_expired(expiration_ms: u64) -> Result<bool> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let safety_ms = EXPIRY_SAFETY_SECS * 1000;
    Ok(now_ms + safety_ms >= expiration_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn caches_credentials_roundtrip() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_CACHE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", temp.path());
        }

        let creds = AwsRoleCredentials {
            access_key_id: "AKIA123".into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            expiration: current_time_ms() + 120_000,
        };
        save_cached_credentials(
            "https://example.awsapps.com/start",
            "us-east-1",
            "1234",
            "Admin",
            &creds,
        )
        .unwrap();
        let loaded = load_cached_credentials(
            "https://example.awsapps.com/start",
            "us-east-1",
            "1234",
            "Admin",
        )
        .unwrap()
        .unwrap();
        assert_eq!(loaded.access_key_id, "AKIA123");
        assert_eq!(loaded.secret_access_key, "secret");

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_CACHE_HOME", value);
            } else {
                std::env::remove_var("XDG_CACHE_HOME");
            }
        }
    }

    #[test]
    fn expired_credentials_are_ignored() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_CACHE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", temp.path());
        }

        let creds = AwsRoleCredentials {
            access_key_id: "AKIA123".into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            expiration: current_time_ms().saturating_sub(120_000),
        };
        save_cached_credentials(
            "https://example.awsapps.com/start",
            "us-east-1",
            "1234",
            "Admin",
            &creds,
        )
        .unwrap();
        let loaded = load_cached_credentials(
            "https://example.awsapps.com/start",
            "us-east-1",
            "1234",
            "Admin",
        )
        .unwrap();
        assert!(loaded.is_none());

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_CACHE_HOME", value);
            } else {
                std::env::remove_var("XDG_CACHE_HOME");
            }
        }
    }

    fn current_time_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}
