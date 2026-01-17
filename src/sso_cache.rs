use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::model::CacheEntry;
use crate::aws_cli;
use tracing::{debug, trace};

pub fn load_valid_cache(start_url: &str) -> Result<CacheEntry> {
    let cache_dir = sso_cache_dir()?;
    let entries = read_cache_entries(&cache_dir, start_url)?;
    let mut best: Option<CacheEntry> = None;
    for entry in entries {
        if is_expired(&entry.expires_at)? {
            continue;
        }
        best = Some(entry);
        break;
    }

    best.ok_or(Error::MissingCache)
}

pub fn device_authorization(start_url: &str, region: &str) -> Result<CacheEntry> {
    debug!(start_url, region, "starting device authorization");
    let client = aws_cli::register_client(region)?;
    trace!(client_id = %client.client_id, "registered client");
    let auth = aws_cli::start_device_authorization(
        region,
        &client.client_id,
        &client.client_secret,
        start_url,
    )?;
    trace!(device_code = %auth.device_code, "received device authorization");

    println!("Open this URL to sign in:\n{}\n", auth.verification_uri_complete);
    println!("Enter code: {}\n", auth.user_code);
    if let Err(err) = open_browser(&auth.verification_uri_complete) {
        debug!(error = %err, "failed to open browser");
    }

    let deadline = SystemTime::now()
        .checked_add(std::time::Duration::from_secs(auth.expires_in))
        .unwrap_or(SystemTime::now());

    loop {
        if SystemTime::now() > deadline {
            return Err(Error::ExpiredCache);
        }

        debug!("polling create-token");
        match aws_cli::create_token(
            region,
            &client.client_id,
            &client.client_secret,
            &auth.device_code,
        ) {
            Ok(token) => {
                let expires_at = SystemTime::now()
                    .checked_add(std::time::Duration::from_secs(token.expires_in))
                    .unwrap_or(SystemTime::now());
                let expires_at = time::OffsetDateTime::from(expires_at)
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
                return Ok(CacheEntry {
                    access_token: token.access_token,
                    expires_at,
                    region: region.to_string(),
                });
            }
            Err(err) => {
                if matches!(err, Error::AwsCliOutput(_)) {
                    std::thread::sleep(std::time::Duration::from_secs(auth.interval.max(1)));
                    continue;
                }
                return Err(err);
            }
        }
    }
}

fn sso_cache_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| Error::MissingHome)?;
    Ok(Path::new(&home).join(".aws").join("sso").join("cache"))
}

fn read_cache_entries(cache_dir: &Path, start_url: &str) -> Result<Vec<CacheEntry>> {
    let mut entries = Vec::new();
    let read_dir = fs::read_dir(cache_dir).map_err(|_| Error::MissingCache)?;
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
    let parsed = time::OffsetDateTime::parse(
        expires_at,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| Error::CacheParse { path: PathBuf::from(expires_at) })?;
    Ok(parsed.unix_timestamp() as u64)
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).status()?;
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).status()?;
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = url;
        return Ok(());
    }
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
