use std::borrow::Cow;
use std::sync::Arc;

use skim::prelude::*;
use tracing::{debug, trace};

use crate::aws_config;
use crate::credentials_cache::{self, CachedCredentialsStatus};
use crate::error::{Error, Result};
use crate::model::RoleChoice;

#[derive(Debug, Clone)]
pub struct TuiSelection {
    pub choice: RoleChoice,
    pub open_in_browser: bool,
}

struct ChoiceItem {
    label: String,
}

impl SkimItem for ChoiceItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.label)
    }
}

pub fn select_role(
    prompt: &str,
    choices: &[RoleChoice],
    start_url: &str,
    region: &str,
) -> Result<Option<TuiSelection>> {
    if choices.is_empty() {
        return Ok(None);
    }

    let mut ordered = choices.to_vec();
    ordered.reverse();
    debug!(count = ordered.len(), "starting role selection");
    eprintln!(
        "{}",
        crate::ui::hint("Type to filter, ↑/↓ to navigate, ⏎ selects, ^o opens in browser.")
    );
    let max_height = std::env::var("LINES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|lines| std::cmp::max(10, lines / 2))
        .unwrap_or(20);
    let height_lines = std::cmp::min(ordered.len().saturating_add(3), max_height);
    let height = format!("{height_lines}");
    let options = SkimOptionsBuilder::default()
        .height(Some(height.as_str()))
        .multi(false)
        .prompt(Some(prompt))
        .bind(vec!["ctrl-c:abort", "ctrl-o:accept"])
        .expect(Some("ctrl-o".to_string()))
        .layout("default")
        .tac(false)
        .reverse(false)
        .nosort(true)
        .build()
        .map_err(|err| Error::Tui(err.to_string()))?;

    let (selected, open_in_browser) = run_skim(&options, &ordered, start_url, region)?;

    if selected.is_empty() {
        debug!("no role selected");
        return Ok(None);
    }

    Ok(Some(TuiSelection {
        choice: selected[0].clone(),
        open_in_browser,
    }))
}

fn run_skim(
    options: &SkimOptions,
    choices: &[RoleChoice],
    start_url: &str,
    region: &str,
) -> Result<(Vec<RoleChoice>, bool)> {
    trace!(count = choices.len(), "preparing skim items");
    let current_profile = std::env::var("AWS_PROFILE").ok();
    let mut lookup = std::collections::HashMap::new();
    let mut items = Vec::with_capacity(choices.len());
    for choice in choices {
        let label = if let Some(profile) = current_profile.as_deref() {
            let prefix = if aws_config::profile_name_for(choice) == profile {
                let status = match credentials_cache::cached_credentials_status(
                    start_url,
                    region,
                    &choice.account_id,
                    &choice.role_name,
                ) {
                    Ok(status) => status,
                    Err(err) => {
                        debug!(error = %err, "failed to check cached credentials");
                        CachedCredentialsStatus::Expired
                    }
                };
                match status {
                    CachedCredentialsStatus::Valid => "* ",
                    CachedCredentialsStatus::Expired | CachedCredentialsStatus::Missing => "! ",
                }
            } else {
                "  "
            };
            format!("{}{}", prefix, choice.label())
        } else {
            choice.label()
        };
        lookup.insert(label.clone(), choice.clone());
        items.push(Arc::new(ChoiceItem { label }) as Arc<dyn SkimItem>);
    }

    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    for item in items {
        if tx.send(item).is_err() {
            break;
        }
    }
    drop(tx);

    let (selected, open_in_browser) = match Skim::run_with(options, Some(rx)) {
        Some(out) => {
            debug!(is_abort = out.is_abort, "skim run completed");
            if out.is_abort {
                (Vec::new(), false)
            } else {
                (out.selected_items, matches!(out.final_key, Key::Ctrl('o')))
            }
        }
        None => {
            debug!("skim returned no output");
            (Vec::new(), false)
        }
    };
    debug!(
        count = selected.len(),
        open_in_browser, "skim selection complete"
    );

    let mut result = Vec::new();
    for item in selected {
        let key = item.text();
        if let Some(choice) = lookup.get(key.as_ref()) {
            result.push(choice.clone());
        } else {
            debug!(value = %key, "missing selection lookup");
        }
    }
    Ok((result, open_in_browser))
}
