use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::roles_cache::roleman_cache_dir;

const PERMISSIONS_CACHE_FILE: &str = "desktop-permissions.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DesktopPermissions {
    #[serde(default)]
    macos_close_auth_tab_authorized: bool,
}

pub(super) fn macos_close_auth_tab_authorized() -> bool {
    match load_permissions() {
        Ok(permissions) => permissions.macos_close_auth_tab_authorized,
        Err(err) => {
            tracing::debug!(error = %err, "failed to load desktop permissions cache");
            false
        }
    }
}

pub(super) fn set_macos_close_auth_tab_authorized(authorized: bool) {
    let mut permissions = match load_permissions() {
        Ok(permissions) => permissions,
        Err(err) => {
            tracing::debug!(error = %err, "failed to load desktop permissions cache");
            DesktopPermissions::default()
        }
    };
    permissions.macos_close_auth_tab_authorized = authorized;
    if let Err(err) = save_permissions(&permissions) {
        tracing::debug!(error = %err, "failed to save desktop permissions cache");
    }
}

fn load_permissions() -> Result<DesktopPermissions> {
    let path = permissions_cache_path()?;
    if !path.exists() {
        return Ok(DesktopPermissions::default());
    }
    let data = fs::read_to_string(&path)
        .map_err(|err| Error::Config(format!("failed to read desktop permissions cache: {err}")))?;
    serde_json::from_str(&data)
        .map_err(|err| Error::Config(format!("failed to parse desktop permissions cache: {err}")))
}

fn save_permissions(permissions: &DesktopPermissions) -> Result<()> {
    let path = permissions_cache_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            Error::Config(format!(
                "failed to create desktop permissions cache directory: {err}"
            ))
        })?;
    }
    let data = serde_json::to_string(permissions).map_err(|err| {
        Error::Config(format!(
            "failed to serialize desktop permissions cache: {err}"
        ))
    })?;
    fs::write(&path, data).map_err(|err| {
        Error::Config(format!("failed to write desktop permissions cache: {err}"))
    })?;
    Ok(())
}

fn permissions_cache_path() -> Result<PathBuf> {
    Ok(roleman_cache_dir()?.join(PERMISSIONS_CACHE_FILE))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        DesktopPermissions, load_permissions, macos_close_auth_tab_authorized,
        permissions_cache_path, save_permissions, set_macos_close_auth_tab_authorized,
    };
    use tempfile::TempDir;

    #[test]
    fn roundtrips_macos_close_auth_tab_permission() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_CACHE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", temp.path());
        }

        assert!(!macos_close_auth_tab_authorized());
        set_macos_close_auth_tab_authorized(true);
        assert!(macos_close_auth_tab_authorized());
        set_macos_close_auth_tab_authorized(false);
        assert!(!macos_close_auth_tab_authorized());

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_CACHE_HOME", value);
            } else {
                std::env::remove_var("XDG_CACHE_HOME");
            }
        }
    }

    #[test]
    fn invalid_cache_defaults_to_not_authorized() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_CACHE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", temp.path());
        }

        let path = permissions_cache_path().unwrap();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, "{invalid json").unwrap();

        assert!(!macos_close_auth_tab_authorized());

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_CACHE_HOME", value);
            } else {
                std::env::remove_var("XDG_CACHE_HOME");
            }
        }
    }

    #[test]
    fn can_load_and_save_permissions_file() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_CACHE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", temp.path());
        }

        let expected = DesktopPermissions {
            macos_close_auth_tab_authorized: true,
        };
        save_permissions(&expected).unwrap();
        let loaded = load_permissions().unwrap();
        assert_eq!(
            loaded.macos_close_auth_tab_authorized,
            expected.macos_close_auth_tab_authorized
        );

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_CACHE_HOME", value);
            } else {
                std::env::remove_var("XDG_CACHE_HOME");
            }
        }
    }
}
