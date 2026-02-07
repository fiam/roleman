use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::SsoIdentity;
use crate::error::{Error, Result};
use crate::model::RoleChoice;

const ROLEMAN_MANAGED_KEY: &str = "roleman_managed";

pub fn profile_name_for(choice: &RoleChoice, omit_role_name: bool) -> String {
    let account = sanitize_component(&choice.account_name);
    if omit_role_name {
        return account;
    }
    let role = sanitize_component(&choice.role_name);
    format!("{}/{}", account, role)
}

pub fn ensure_sso_session(identity: &SsoIdentity) -> Result<String> {
    let session = sso_session_name(identity);
    let entries = vec![
        ("sso_start_url", identity.start_url.as_str()),
        ("sso_region", identity.sso_region.as_str()),
    ];
    ensure_section_entries(&format!("sso-session {session}"), &entries)?;
    Ok(session)
}

pub fn ensure_role_profile(
    profile_name: &str,
    choice: &RoleChoice,
    identity: &SsoIdentity,
    region: &str,
) -> Result<()> {
    let session = sso_session_name(identity);
    let entries = vec![
        ("sso_session", session.as_str()),
        ("sso_account_id", choice.account_id.as_str()),
        ("sso_role_name", choice.role_name.as_str()),
        ("region", region),
        (ROLEMAN_MANAGED_KEY, "true"),
    ];
    ensure_profile_entries(profile_name, &entries)
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

pub fn aws_config_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| Error::MissingHome)?;
    Ok(Path::new(&home).join(".aws").join("config"))
}

fn sso_session_name(identity: &SsoIdentity) -> String {
    format!("roleman-{}", sanitize_component(&identity.name))
}

fn ensure_profile_entries(profile: &str, entries: &[(&str, &str)]) -> Result<()> {
    ensure_section_entries(&format!("profile {profile}"), entries)
}

fn find_section(lines: &[String], header: &str) -> (Option<usize>, Option<usize>) {
    let mut start = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Some(section_start) = start {
                return (Some(section_start), Some(idx));
            }
            if trimmed == header {
                start = Some(idx);
            }
        }
    }
    (start, None)
}

fn parse_key_value(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
        return None;
    }
    let (key, value) = trimmed.split_once('=')?;
    Some((key.trim().to_string(), value.trim().to_string()))
}

fn is_truthy(value: &str) -> bool {
    matches!(value.trim().to_lowercase().as_str(), "true" | "1" | "yes")
}

fn ensure_section_entries(section: &str, entries: &[(&str, &str)]) -> Result<()> {
    let path = aws_config_path()?;
    let header = format!("[{}]", section);

    let contents = fs::read_to_string(&path).unwrap_or_default();
    let had_trailing_newline = contents.ends_with('\n');
    let mut lines: Vec<String> = if contents.is_empty() {
        Vec::new()
    } else {
        contents.lines().map(|line| line.to_string()).collect()
    };

    let (start, end) = find_section(&lines, &header);
    if let Some(start) = start {
        let end = end.unwrap_or(lines.len());
        let mut key_lines: HashMap<String, usize> = HashMap::new();
        let mut key_values: HashMap<String, String> = HashMap::new();
        for (idx, line) in lines.iter().enumerate().take(end).skip(start + 1) {
            if let Some((key, value)) = parse_key_value(line) {
                key_lines.insert(key.clone(), idx);
                key_values.insert(key, value);
            }
        }
        let roleman_value = key_values.get(ROLEMAN_MANAGED_KEY);
        let managed = roleman_value.map(|value| is_truthy(value)).unwrap_or(false);
        if let Some(value) = roleman_value
            && !is_truthy(value)
        {
            return Err(Error::Config(format!(
                "section {section} already exists and is not managed by roleman"
            )));
        }

        if !managed {
            let mut missing_required = Vec::new();
            for (key, desired) in entries {
                if *key == ROLEMAN_MANAGED_KEY {
                    continue;
                }
                match key_values.get(*key) {
                    Some(existing) if existing == desired => {}
                    Some(_) => {
                        return Err(Error::Config(format!(
                            "section {section} already exists and is not managed by roleman"
                        )));
                    }
                    None => missing_required.push(*key),
                }
            }
            if !missing_required.is_empty() {
                return Err(Error::Config(format!(
                    "section {section} already exists and is not managed by roleman"
                )));
            }
        }

        for (key, value) in entries {
            if let Some(idx) = key_lines.get(*key) {
                lines[*idx] = format!("{key} = {value}");
            }
        }

        let missing = entries
            .iter()
            .filter(|(key, _)| !key_lines.contains_key(*key))
            .map(|(key, value)| format!("{key} = {value}"))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            let mut out = Vec::with_capacity(lines.len() + missing.len());
            for (idx, line) in lines.iter().enumerate() {
                if idx == end {
                    out.extend(missing.iter().cloned());
                }
                out.push(line.clone());
            }
            if end == lines.len() {
                out.extend(missing);
            }
            lines = out;
        }
    } else {
        if !lines.is_empty() && lines.last().is_some_and(|line| !line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(header);
        for (key, value) in entries {
            lines.push(format!("{key} = {value}"));
        }
    }

    let mut output = lines.join("\n");
    if had_trailing_newline || (!output.is_empty() && !output.ends_with('\n')) {
        output.push('\n');
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| Error::Config(err.to_string()))?;
    }
    fs::write(&path, output).map_err(|err| Error::Config(err.to_string()))
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
    fn omits_role_name_for_single_role_accounts() {
        let choice = RoleChoice {
            account_id: "1234".into(),
            account_name: "Acme Cloud".into(),
            role_name: "ReadOnly".into(),
        };
        assert_eq!(profile_name_for(&choice, true), "Acme-Cloud");
        assert_eq!(profile_name_for(&choice, false), "Acme-Cloud/ReadOnly");
    }

    #[test]
    fn ensures_role_profile() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", temp.path());
        }

        let identity = SsoIdentity {
            name: "work".into(),
            start_url: "https://example.awsapps.com/start".into(),
            sso_region: "us-east-1".into(),
            accounts: Vec::new(),
            ignore_roles: Vec::new(),
        };
        let choice = RoleChoice {
            account_id: "1234".into(),
            account_name: "Acme Cloud".into(),
            role_name: "ReadOnly".into(),
        };
        let session = ensure_sso_session(&identity).unwrap();
        assert_eq!(session, "roleman-work");
        let profile_name = profile_name_for(&choice, false);
        ensure_role_profile(&profile_name, &choice, &identity, "us-east-1").unwrap();
        let config_path = aws_config_path().unwrap();
        let contents = fs::read_to_string(config_path).unwrap();
        assert!(contents.contains("[sso-session roleman-work]"));
        assert!(contents.contains("sso_start_url = https://example.awsapps.com/start"));
        assert!(contents.contains("sso_region = us-east-1"));
        assert!(contents.contains("[profile Acme-Cloud/ReadOnly]"));
        assert!(contents.contains("sso_session = roleman-work"));
        assert!(contents.contains("sso_account_id = 1234"));
        assert!(contents.contains("sso_role_name = ReadOnly"));
        assert!(contents.contains("region = us-east-1"));
        assert!(contents.contains("roleman_managed = true"));

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("HOME", value);
            } else {
                std::env::remove_var("HOME");
            }
        }
    }
}
