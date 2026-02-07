use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::model::CacheEntry;
use crate::ui;
use tracing::debug;

pub fn load_valid_cache(start_url: &str) -> Result<CacheEntry> {
    let aws_cache_dir = aws_sso_cache_dir()?;
    let entries = read_cache_entries_from_dir(&aws_cache_dir, start_url)?;
    let mut best: Option<(CacheEntry, u64)> = None;
    for entry in entries {
        if is_expired(&entry.expires_at)? {
            continue;
        }
        let expires_epoch = aws_time_to_epoch(&entry.expires_at)?;
        let should_replace = match &best {
            Some((_, best_epoch)) => expires_epoch > *best_epoch,
            None => true,
        };
        if should_replace {
            best = Some((entry, expires_epoch));
        }
    }

    if let Some((entry, _)) = best {
        let remaining = time_until_expiry(&entry.expires_at).unwrap_or_default();
        debug!(expires_at = %entry.expires_at, "using cached sso token");
        eprintln!(
            "{}",
            ui::info(&format!(
                "Using cached SSO token (valid for {}).",
                format_duration(remaining)
            ))
        );
        return Ok(entry);
    }
    Err(Error::MissingCache)
}

fn aws_sso_cache_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| Error::MissingHome)?;
    Ok(Path::new(&home).join(".aws").join("sso").join("cache"))
}

fn read_cache_entries_from_dir(dir: &Path, start_url: &str) -> Result<Vec<CacheEntry>> {
    let mut entries = Vec::new();
    let read_dir = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(_) => return Ok(entries),
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let data = match fs::read_to_string(&path) {
            Ok(data) => data,
            Err(_) => continue,
        };
        let value: serde_json::Value = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let start = value
            .get("startUrl")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if start != start_url {
            continue;
        }
        let access_token = value.get("accessToken").and_then(|v| v.as_str());
        let region = value.get("region").and_then(|v| v.as_str());
        let expires_at = value.get("expiresAt").and_then(|v| v.as_str());
        if let (Some(access_token), Some(region), Some(expires_at)) =
            (access_token, region, expires_at)
        {
            entries.push(CacheEntry {
                access_token: access_token.to_string(),
                expires_at: expires_at.to_string(),
                region: region.to_string(),
            });
        }
    }
    Ok(entries)
}

fn is_expired(expires_at: &str) -> Result<bool> {
    let expires_epoch = aws_time_to_epoch(expires_at)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(now >= expires_epoch)
}

fn aws_time_to_epoch(expires_at: &str) -> Result<u64> {
    let parsed =
        time::OffsetDateTime::parse(expires_at, &time::format_description::well_known::Rfc3339)
            .map_err(|_| Error::CacheParse {
                path: PathBuf::from(expires_at),
            })?;
    Ok(parsed.unix_timestamp() as u64)
}

fn time_until_expiry(expires_at: &str) -> Result<std::time::Duration> {
    let expires_epoch = aws_time_to_epoch(expires_at)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if expires_epoch <= now {
        return Ok(std::time::Duration::from_secs(0));
    }
    Ok(std::time::Duration::from_secs(expires_epoch - now))
}

fn format_duration(duration: std::time::Duration) -> String {
    let total = duration.as_secs();
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parses_expiration() {
        let epoch = aws_time_to_epoch("2099-01-01T00:00:00Z").unwrap();
        assert!(epoch > 0);
    }

    #[test]
    fn loads_cached_token_from_aws_cache() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", temp.path());
        }
        let cache_dir = aws_sso_cache_dir().unwrap();
        fs::create_dir_all(&cache_dir).unwrap();
        let expires_at = time::OffsetDateTime::from(
            SystemTime::now()
                .checked_add(std::time::Duration::from_secs(600))
                .unwrap(),
        )
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
        let payload = serde_json::json!({
            "startUrl": "https://example.awsapps.com/start",
            "region": "us-east-1",
            "accessToken": "token",
            "expiresAt": expires_at,
        });
        let path = cache_dir.join("cache.json");
        fs::write(path, payload.to_string()).unwrap();

        let loaded = load_valid_cache("https://example.awsapps.com/start").unwrap();
        assert_eq!(loaded.access_token, "token");

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("HOME", value);
            } else {
                std::env::remove_var("HOME");
            }
        }
    }
}
