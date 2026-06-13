use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::error::{Error, Result};
use crate::provider::AccessScope;
use crate::roles_cache::roleman_cache_dir;

const EXPIRY_SAFETY_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachedCredentialsStatus {
    Valid,
    Expired,
    Missing,
}

/// On-disk envelope: an expiration we can check without parsing the opaque payload,
/// plus the provider-specific credential JSON it produced.
#[derive(Debug, Serialize, Deserialize)]
struct CachedCredentials {
    expiration_ms: u64,
    payload: String,
}

/// Load the cached credential payload for a target, or `None` if missing/expired.
pub fn load_cached_payload(
    namespace: &str,
    account_id: &str,
    role_name: &str,
    scope: AccessScope,
) -> Result<Option<String>> {
    let Some(cached) = read(namespace, account_id, role_name, scope)? else {
        return Ok(None);
    };
    if is_expired(cached.expiration_ms)? {
        return Ok(None);
    }
    Ok(Some(cached.payload))
}

pub fn cached_credentials_status(
    namespace: &str,
    account_id: &str,
    role_name: &str,
    scope: AccessScope,
) -> Result<CachedCredentialsStatus> {
    let Some(cached) = read(namespace, account_id, role_name, scope)? else {
        return Ok(CachedCredentialsStatus::Missing);
    };
    if is_expired(cached.expiration_ms)? {
        Ok(CachedCredentialsStatus::Expired)
    } else {
        Ok(CachedCredentialsStatus::Valid)
    }
}

pub fn save_cached_payload(
    namespace: &str,
    account_id: &str,
    role_name: &str,
    scope: AccessScope,
    expiration_ms: u64,
    payload: &str,
) -> Result<()> {
    let path = cache_path(namespace, account_id, role_name, scope)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| Error::MissingCache)?;
    }
    let cached = CachedCredentials {
        expiration_ms,
        payload: payload.to_string(),
    };
    let data =
        serde_json::to_string(&cached).map_err(|_| Error::CacheParse { path: path.clone() })?;
    fs::write(&path, data).map_err(|_| Error::CacheParse { path })?;
    Ok(())
}

fn read(
    namespace: &str,
    account_id: &str,
    role_name: &str,
    scope: AccessScope,
) -> Result<Option<CachedCredentials>> {
    let path = cache_path(namespace, account_id, role_name, scope)?;
    if !path.exists() {
        return Ok(None);
    }
    let data = match fs::read_to_string(&path) {
        Ok(data) => data,
        Err(_) => return Ok(None),
    };
    Ok(serde_json::from_str(&data).ok())
}

fn cache_path(
    namespace: &str,
    account_id: &str,
    role_name: &str,
    scope: AccessScope,
) -> Result<PathBuf> {
    let cache_dir = roleman_cache_dir()?;
    Ok(cache_dir.join(cache_filename(namespace, account_id, role_name, scope)))
}

fn cache_filename(
    namespace: &str,
    account_id: &str,
    role_name: &str,
    scope: AccessScope,
) -> String {
    let mut hasher = Sha1::new();
    hasher.update(namespace.as_bytes());
    hasher.update(account_id.as_bytes());
    hasher.update(role_name.as_bytes());
    hasher.update(scope.cache_tag().as_bytes());
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

    fn current_time_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    #[test]
    fn caches_payload_roundtrip() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_CACHE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", temp.path());
        }

        save_cached_payload(
            "work",
            "1234",
            "Admin",
            AccessScope::Full,
            current_time_ms() + 120_000,
            "{\"token\":\"abc\"}",
        )
        .unwrap();
        let loaded = load_cached_payload("work", "1234", "Admin", AccessScope::Full).unwrap();
        assert_eq!(loaded.as_deref(), Some("{\"token\":\"abc\"}"));

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_CACHE_HOME", value);
            } else {
                std::env::remove_var("XDG_CACHE_HOME");
            }
        }
    }

    #[test]
    fn scopes_cache_separately() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_CACHE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", temp.path());
        }

        save_cached_payload(
            "work",
            "1234",
            "Admin",
            AccessScope::Full,
            current_time_ms() + 120_000,
            "full",
        )
        .unwrap();
        // ReadOnly scope must not see the Full-scope entry.
        let readonly = load_cached_payload("work", "1234", "Admin", AccessScope::ReadOnly).unwrap();
        assert!(readonly.is_none());

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_CACHE_HOME", value);
            } else {
                std::env::remove_var("XDG_CACHE_HOME");
            }
        }
    }

    #[test]
    fn expired_payload_is_ignored() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_CACHE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", temp.path());
        }

        save_cached_payload(
            "work",
            "1234",
            "Admin",
            AccessScope::Full,
            current_time_ms().saturating_sub(120_000),
            "stale",
        )
        .unwrap();
        let loaded = load_cached_payload("work", "1234", "Admin", AccessScope::Full).unwrap();
        assert!(loaded.is_none());
        let status = cached_credentials_status("work", "1234", "Admin", AccessScope::Full).unwrap();
        assert_eq!(status, CachedCredentialsStatus::Expired);

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_CACHE_HOME", value);
            } else {
                std::env::remove_var("XDG_CACHE_HOME");
            }
        }
    }
}
