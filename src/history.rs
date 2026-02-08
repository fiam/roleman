use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::debug;

use crate::error::{Error, Result};
use crate::model::RoleChoice;

const RECENCY_DECAY_DAYS: f64 = 14.0;
const FREQUENCY_WINDOW_DAYS: i64 = 30;
const RECENCY_WEIGHT: f64 = 0.60;
const FREQUENCY_WEIGHT: f64 = 0.30;
const CONTEXT_WEIGHT: f64 = 0.10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub selected_at_unix: i64,
    pub identity: String,
    pub account_id: String,
    pub account_name: String,
    pub role_name: String,
    #[serde(default, alias = "cwd_hash")]
    pub cwd: Option<String>,
}

#[derive(Debug, Default)]
struct HistoryStats {
    recency_score: f64,
    frequency_30d: u32,
    cwd_matches: bool,
}

pub fn record_selection(identity: &str, choice: &RoleChoice) -> Result<()> {
    let entry = HistoryEntry {
        selected_at_unix: OffsetDateTime::now_utc().unix_timestamp(),
        identity: identity.to_string(),
        account_id: choice.account_id.clone(),
        account_name: choice.account_name.clone(),
        role_name: choice.role_name.clone(),
        cwd: current_cwd(),
    };

    let path = history_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| Error::Config(err.to_string()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| Error::Config(err.to_string()))?;
    let serialized = serde_json::to_string(&entry).map_err(|err| Error::Config(err.to_string()))?;
    writeln!(file, "{serialized}").map_err(|err| Error::Config(err.to_string()))
}

pub fn apply_history_sort(
    choices: &mut [RoleChoice],
    identity: &str,
    initial_query: Option<&str>,
) -> Result<()> {
    if initial_query
        .map(str::trim)
        .is_some_and(|query| !query.is_empty())
    {
        return Ok(());
    }

    let entries = load_entries()?;
    if entries.is_empty() {
        return Ok(());
    }

    sort_choices_with_history(
        choices,
        identity,
        &entries,
        OffsetDateTime::now_utc().unix_timestamp(),
        current_cwd().as_deref(),
    );
    Ok(())
}

pub fn recent_entries(limit: usize) -> Result<Vec<HistoryEntry>> {
    let mut entries = load_entries()?;
    entries.sort_by(|left, right| right.selected_at_unix.cmp(&left.selected_at_unix));
    entries.truncate(limit);
    Ok(entries)
}

pub fn clear_entries() -> Result<()> {
    let path = history_path()?;
    if path.exists() {
        fs::remove_file(path).map_err(|err| Error::Config(err.to_string()))?;
    }
    Ok(())
}

pub fn history_path() -> Result<PathBuf> {
    let base = if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        PathBuf::from(dir)
    } else {
        let home = std::env::var("HOME").map_err(|_| Error::MissingHome)?;
        PathBuf::from(home).join(".local").join("state")
    };
    Ok(base.join("roleman").join("history.jsonl"))
}

pub fn format_entry(entry: &HistoryEntry) -> String {
    let timestamp = format_timestamp(entry.selected_at_unix);
    let cwd = entry.cwd.as_deref().unwrap_or("-");
    format!(
        "{timestamp}\t{}\t{}\t{}\t{}",
        entry.identity, entry.account_id, entry.role_name, cwd
    )
}

pub fn format_timestamp(unix_timestamp: i64) -> String {
    OffsetDateTime::from_unix_timestamp(unix_timestamp)
        .ok()
        .and_then(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| unix_timestamp.to_string())
}

fn load_entries() -> Result<Vec<HistoryEntry>> {
    let path = history_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    load_entries_from_path(&path)
}

fn load_entries_from_path(path: &Path) -> Result<Vec<HistoryEntry>> {
    let file = File::open(path).map_err(|err| Error::Config(err.to_string()))?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();

    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|err| Error::Config(err.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<HistoryEntry>(&line) {
            Ok(entry) => entries.push(entry),
            Err(err) => {
                debug!(
                    path = %path.display(),
                    line_number = index + 1,
                    error = %err,
                    "skipping malformed history entry"
                );
            }
        }
    }

    Ok(entries)
}

fn sort_choices_with_history(
    choices: &mut [RoleChoice],
    identity: &str,
    entries: &[HistoryEntry],
    now_unix: i64,
    cwd: Option<&str>,
) {
    let stats = build_stats(entries, identity, now_unix, cwd);
    if stats.is_empty() {
        return;
    }
    choices.sort_by(|left, right| {
        let left_score = score_for_choice(&stats, left);
        let right_score = score_for_choice(&stats, right);
        right_score.total_cmp(&left_score)
    });
}

fn build_stats(
    entries: &[HistoryEntry],
    identity: &str,
    now_unix: i64,
    cwd: Option<&str>,
) -> HashMap<(String, String), HistoryStats> {
    let mut stats = HashMap::new();

    for entry in entries.iter().filter(|entry| entry.identity == identity) {
        let key = (entry.account_id.clone(), entry.role_name.clone());
        let account_stats = stats.entry(key).or_insert_with(HistoryStats::default);
        let age_seconds = now_unix.saturating_sub(entry.selected_at_unix);
        let age_days = age_seconds as f64 / 86_400.0;
        let recency = (-age_days / RECENCY_DECAY_DAYS).exp();
        account_stats.recency_score = account_stats.recency_score.max(recency);
        if age_seconds <= FREQUENCY_WINDOW_DAYS * 86_400 {
            account_stats.frequency_30d = account_stats.frequency_30d.saturating_add(1);
        }
        if let Some(cwd) = cwd
            && entry.cwd.as_deref() == Some(cwd)
        {
            account_stats.cwd_matches = true;
        }
    }

    stats
}

fn score_for_choice(stats: &HashMap<(String, String), HistoryStats>, choice: &RoleChoice) -> f64 {
    let key = (choice.account_id.clone(), choice.role_name.clone());
    let Some(stats) = stats.get(&key) else {
        return 0.0;
    };
    let frequency = ((stats.frequency_30d as f64) + 1.0).ln() / 31.0_f64.ln();
    let context = if stats.cwd_matches { 1.0 } else { 0.0 };
    stats.recency_score * RECENCY_WEIGHT + frequency * FREQUENCY_WEIGHT + context * CONTEXT_WEIGHT
}

fn current_cwd() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let canonical = cwd.canonicalize().unwrap_or(cwd);
    Some(canonical.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn history_path_uses_xdg_state_home() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_STATE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp.path());
        }

        let path = history_path().unwrap();
        assert_eq!(path, temp.path().join("roleman").join("history.jsonl"));

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_STATE_HOME", value);
            } else {
                std::env::remove_var("XDG_STATE_HOME");
            }
        }
    }

    #[test]
    fn records_and_reads_entries() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_STATE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp.path());
        }

        let choice = RoleChoice {
            account_id: "111111111111".into(),
            account_name: "Payments".into(),
            role_name: "Admin".into(),
        };
        record_selection("work", &choice).unwrap();

        let entries = recent_entries(10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].identity, "work");
        assert_eq!(entries[0].account_id, "111111111111");
        assert_eq!(entries[0].role_name, "Admin");
        assert!(entries[0].cwd.is_some());

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_STATE_HOME", value);
            } else {
                std::env::remove_var("XDG_STATE_HOME");
            }
        }
    }

    #[test]
    fn skips_invalid_lines_while_loading() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("history.jsonl");
        std::fs::write(
            &path,
            r#"{"selected_at_unix":1,"identity":"work","account_id":"111","account_name":"A","role_name":"Admin","cwd":null}
not-json
"#,
        )
        .unwrap();

        let entries = load_entries_from_path(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].account_id, "111");
    }

    #[test]
    fn loads_legacy_cwd_hash_field() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("history.jsonl");
        std::fs::write(
            &path,
            r#"{"selected_at_unix":1,"identity":"work","account_id":"111","account_name":"A","role_name":"Admin","cwd_hash":"legacy"}
"#,
        )
        .unwrap();

        let entries = load_entries_from_path(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cwd.as_deref(), Some("legacy"));
    }

    #[test]
    fn applies_history_sort_with_context_boost() {
        let now = 1_700_000_000;
        let mut choices = vec![
            RoleChoice {
                account_id: "111".into(),
                account_name: "A".into(),
                role_name: "Admin".into(),
            },
            RoleChoice {
                account_id: "222".into(),
                account_name: "B".into(),
                role_name: "Admin".into(),
            },
        ];
        let entries = vec![
            HistoryEntry {
                selected_at_unix: now - (10 * 86_400),
                identity: "work".into(),
                account_id: "111".into(),
                account_name: "A".into(),
                role_name: "Admin".into(),
                cwd: Some("/tmp/cwd-a".into()),
            },
            HistoryEntry {
                selected_at_unix: now - (8 * 86_400),
                identity: "work".into(),
                account_id: "222".into(),
                account_name: "B".into(),
                role_name: "Admin".into(),
                cwd: Some("/tmp/cwd-b".into()),
            },
        ];

        sort_choices_with_history(&mut choices, "work", &entries, now, Some("/tmp/cwd-a"));
        assert_eq!(choices[0].account_id, "111");
    }

    #[test]
    fn score_combines_recency_frequency_and_context() {
        let choice = RoleChoice {
            account_id: "111".into(),
            account_name: "A".into(),
            role_name: "Admin".into(),
        };
        let mut stats = std::collections::HashMap::new();
        stats.insert(
            ("111".to_string(), "Admin".to_string()),
            HistoryStats {
                recency_score: 0.75,
                frequency_30d: 9,
                cwd_matches: true,
            },
        );

        let actual = score_for_choice(&stats, &choice);
        let frequency_component = (10.0_f64).ln() / 31.0_f64.ln();
        let expected =
            0.75 * RECENCY_WEIGHT + frequency_component * FREQUENCY_WEIGHT + CONTEXT_WEIGHT;

        assert!((actual - expected).abs() < 1e-12);

        let missing_choice = RoleChoice {
            account_id: "999".into(),
            account_name: "B".into(),
            role_name: "ReadOnly".into(),
        };
        assert_eq!(score_for_choice(&stats, &missing_choice), 0.0);
    }

    #[test]
    fn clear_entries_removes_file() {
        let _lock = crate::test_support::lock_env();
        let temp = TempDir::new().unwrap();
        let previous = std::env::var("XDG_STATE_HOME").ok();
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp.path());
        }

        let path = history_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "test\n").unwrap();
        clear_entries().unwrap();
        assert!(!path.exists());

        unsafe {
            if let Some(value) = previous {
                std::env::set_var("XDG_STATE_HOME", value);
            } else {
                std::env::remove_var("XDG_STATE_HOME");
            }
        }
    }
}
