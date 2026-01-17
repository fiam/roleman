use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::model::RoleChoice;

pub fn profile_name_for(choice: &RoleChoice) -> String {
    let account = sanitize_component(&choice.account_name);
    let role = sanitize_component(&choice.role_name);
    format!("{}/{}", account, role)
}

pub fn ensure_profile_region(profile: &str, region: &str) -> Result<PathBuf> {
    let path = roleman_aws_config_path()?;
    let section = if profile == "default" {
        "default".to_string()
    } else {
        format!("profile {}", profile)
    };
    let header = format!("[{}]", section);

    let mut contents = fs::read_to_string(&path).unwrap_or_default();
    if contents.lines().any(|line| line.trim() == header) {
        return Ok(path);
    }
    if !contents.ends_with('\n') && !contents.is_empty() {
        contents.push('\n');
    }
    contents.push_str(&header);
    contents.push('\n');
    contents.push_str(&format!("region={}\n", region));
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
        .map_err(|err| Error::Config(err.to_string()))?;
    fs::write(&path, contents).map_err(|err| Error::Config(err.to_string()))?;
    Ok(path)
}

fn sanitize_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "role".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn sanitizes_profile_components() {
        assert_eq!(sanitize_component("Acme Cloud/Prod"), "Acme-Cloud-Prod");
    }

    #[test]
    fn ensures_config_profile() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_STATE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp.path());
        }

        ensure_profile_region("Acme-Cloud/ReadOnly", "us-east-1").unwrap();
        let config_path = roleman_aws_config_path().unwrap();
        let contents = fs::read_to_string(config_path).unwrap();
        assert!(contents.contains("[profile Acme-Cloud/ReadOnly]"));
        assert!(contents.contains("region=us-east-1"));

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_STATE_HOME", value);
            } else {
                std::env::remove_var("XDG_STATE_HOME");
            }
        }
    }
}

fn roleman_aws_config_path() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        Ok(PathBuf::from(dir).join("roleman").join("aws-config"))
    } else {
        let home = std::env::var("HOME").map_err(|_| Error::MissingHome)?;
        Ok(Path::new(&home).join(".local").join("state").join("roleman").join("aws-config"))
    }
}
