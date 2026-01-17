use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha1::{Digest, Sha1};
use crate::aws_sdk;
use crate::error::{Error, Result};
use crate::model::CacheEntry;
use crate::ui;
use tracing::{debug, trace};

pub fn load_valid_cache(start_url: &str) -> Result<CacheEntry> {
    let aws_cache_dir = aws_sso_cache_dir()?;
    let roleman_cache_dir = roleman_cache_dir()?;
    let entries = read_cache_entries_from_dirs(&[aws_cache_dir, roleman_cache_dir], start_url)?;
    let mut best: Option<CacheEntry> = None;
    for entry in entries {
        if is_expired(&entry.expires_at)? {
            continue;
        }
        best = Some(entry);
        break;
    }

    if let Some(entry) = best.clone() {
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

pub async fn device_authorization(start_url: &str, region: &str) -> Result<CacheEntry> {
    debug!(start_url, region, "starting device authorization");
    eprintln!("{}", ui::action("Starting SSO device authorization..."));
    let client = aws_sdk::register_client(region).await?;
    trace!(client_id = %client.client_id, "registered client");
    let auth = aws_sdk::start_device_authorization(
        region,
        &client.client_id,
        &client.client_secret,
        start_url,
    )
    .await?;
    trace!(device_code = %auth.device_code, "received device authorization");

    eprintln!(
        "{}",
        ui::action(&format!(
            "Open this URL to sign in:\n{}\n",
            auth.verification_uri_complete
        ))
    );
    eprintln!("🔐 {}", auth.user_code);
    if let Err(err) = open_browser(&auth.verification_uri_complete) {
        debug!(error = %err, "failed to open browser");
    }
    eprintln!("{}", ui::action("Waiting for authorization to complete..."));

    let deadline = SystemTime::now()
        .checked_add(std::time::Duration::from_secs(auth.expires_in))
        .unwrap_or(SystemTime::now());
    let mut last_feedback = SystemTime::now();

    loop {
        if SystemTime::now() > deadline {
            return Err(Error::ExpiredCache);
        }

        debug!("polling create-token");
        if last_feedback.elapsed().unwrap_or_default().as_secs() >= 5 {
            eprintln!("{}", ui::info("Still waiting for device authorization..."));
            last_feedback = SystemTime::now();
        }
        match aws_sdk::create_token(
            region,
            &client.client_id,
            &client.client_secret,
            &auth.device_code,
        )
        .await
        {
            Ok(token) => {
                eprintln!("{}", ui::success("Authorization complete, fetching access token..."));
                let expires_at = SystemTime::now()
                    .checked_add(std::time::Duration::from_secs(token.expires_in))
                    .unwrap_or(SystemTime::now());
                let expires_at = time::OffsetDateTime::from(expires_at)
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
                let entry = CacheEntry {
                    access_token: token.access_token,
                    expires_at,
                    region: region.to_string(),
                };
                write_cache_entry(start_url, &entry)?;
                eprintln!("{}", ui::success("Access token cached."));
                return Ok(entry);
            }
            Err(err) => {
                if is_pending_auth(&err) {
                    tokio::time::sleep(std::time::Duration::from_secs(auth.interval.max(1))).await;
                    continue;
                }
                return Err(err);
            }
        }
    }
}

fn aws_sso_cache_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| Error::MissingHome)?;
    Ok(Path::new(&home).join(".aws").join("sso").join("cache"))
}

fn roleman_cache_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        Ok(PathBuf::from(dir).join("roleman"))
    } else {
        let home = std::env::var("HOME").map_err(|_| Error::MissingHome)?;
        Ok(Path::new(&home).join(".cache").join("roleman"))
    }
}

fn read_cache_entries_from_dirs(
    cache_dirs: &[PathBuf],
    start_url: &str,
) -> Result<Vec<CacheEntry>> {
    let mut entries = Vec::new();
    for cache_dir in cache_dirs {
        let read_dir = match fs::read_dir(cache_dir) {
            Ok(read_dir) => read_dir,
            Err(_) => continue,
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
    let parsed = time::OffsetDateTime::parse(
        expires_at,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| Error::CacheParse { path: PathBuf::from(expires_at) })?;
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

fn is_pending_auth(error: &Error) -> bool {
    match error {
        Error::AwsSdk(message) => {
            message.contains("AuthorizationPendingException")
                || message.contains("SlowDownException")
                || message.contains("InvalidGrantException")
        }
        _ => false,
    }
}

fn write_cache_entry(start_url: &str, entry: &CacheEntry) -> Result<()> {
    let cache_dir = roleman_cache_dir()?;
    fs::create_dir_all(&cache_dir).map_err(|_| Error::MissingCache)?;
    let path = cache_dir.join(cache_filename(start_url));
    let value = serde_json::json!({
        "startUrl": start_url,
        "region": entry.region,
        "accessToken": entry.access_token,
        "expiresAt": entry.expires_at,
    });
    let data = serde_json::to_string(&value).map_err(|_| Error::CacheParse { path: path.clone() })?;
    fs::write(&path, data).map_err(|_| Error::CacheParse { path })?;
    Ok(())
}

fn cache_filename(start_url: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(start_url.as_bytes());
    let digest = hasher.finalize();
    format!("roleman-{:x}.json", digest)
}

fn open_browser(url: &str) -> std::io::Result<()> {
    open::that(url).map(|_| ()).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_expiration() {
        let epoch = aws_time_to_epoch("2099-01-01T00:00:00Z").unwrap();
        assert!(epoch > 0);
    }
}
