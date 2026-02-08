use std::borrow::Cow;
use std::sync::Arc;

use skim::matcher::Matcher;
use skim::prelude::*;
use skim::tui::event::Action;
use skim::tui::options::TuiLayout;
use skim::tui::statusline::InfoDisplay;
use tracing::{debug, trace};

use crate::aws_config;
use crate::credentials_cache::{self, CachedCredentialsStatus};
use crate::error::{Error, Result};
use crate::model::RoleChoice;

const SKIM_COLOR_OVERRIDES: &str =
    "dark,matched-bg:-1,bg+:-1,current:-1:reverse,current_match-bg:-1,current_match:-1:reverse";

#[derive(Debug, Clone)]
pub struct TuiSelection {
    pub choice: RoleChoice,
    pub open_in_browser: bool,
    pub auto_selected: bool,
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
    initial_query: Option<&str>,
) -> Result<Option<TuiSelection>> {
    if choices.is_empty() {
        return Ok(None);
    }

    let mut ordered = choices.to_vec();
    ordered.reverse();
    debug!(count = ordered.len(), "starting role selection");
    let initial_query = normalize_initial_query(initial_query);
    let max_height = std::env::var("LINES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|lines| std::cmp::max(10, lines / 2))
        .unwrap_or(20);
    let height_lines = std::cmp::min(ordered.len().saturating_add(3), max_height);
    let height = format!("{height_lines}");
    let mut options_builder = SkimOptionsBuilder::default();
    options_builder
        .height(height)
        .multi(false)
        .prompt(prompt.to_string())
        // Use reverse-video for the current row so selection stays visible on light/dark terminals.
        .color(Some(SKIM_COLOR_OVERRIDES.to_string()))
        .info(InfoDisplay::Hidden)
        .bind(vec![
            "ctrl-c:abort".to_string(),
            "ctrl-o:accept(ctrl-o)".to_string(),
        ])
        .layout(TuiLayout::Default)
        .sync(true)
        .tac(false)
        .reverse(false)
        .no_sort(true);
    if let Some(query) = initial_query.as_deref() {
        options_builder.query(Some(query.to_string()));
    }
    let options = options_builder
        .build()
        .map_err(|err| Error::Tui(err.to_string()))?;

    if let Some(query) = initial_query.as_deref()
        && let Some(choice) = find_single_query_match(&options, &ordered, query)
    {
        debug!(query, choice = %choice.label(), "auto-selected role from initial query");
        return Ok(Some(TuiSelection {
            choice,
            open_in_browser: false,
            auto_selected: true,
        }));
    }

    eprintln!(
        "{}",
        crate::ui::hint("Type to filter, ↑/↓ to navigate, ⏎ selects, ^o opens in browser.")
    );

    let (selected, open_in_browser) = run_skim(options, &ordered, start_url, region)?;

    if selected.is_empty() {
        debug!("no role selected");
        return Ok(None);
    }

    Ok(Some(TuiSelection {
        choice: selected[0].clone(),
        open_in_browser,
        auto_selected: false,
    }))
}

fn normalize_initial_query(initial_query: Option<&str>) -> Option<String> {
    initial_query
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(ToOwned::to_owned)
}

fn find_single_query_match(
    options: &SkimOptions,
    choices: &[RoleChoice],
    query: &str,
) -> Option<RoleChoice> {
    let engine_factory = Matcher::create_engine_factory(options);
    let engine = engine_factory.create_engine_with_case(query, options.case);
    let mut matches = choices.iter().filter(|choice| {
        let item: Arc<dyn SkimItem> = Arc::new(ChoiceItem {
            label: choice.label(),
        });
        engine.match_item(item).is_some()
    });
    let first = matches.next()?.clone();
    if matches.next().is_some() {
        return None;
    }
    Some(first)
}

fn run_skim(
    options: SkimOptions,
    choices: &[RoleChoice],
    start_url: &str,
    region: &str,
) -> Result<(Vec<RoleChoice>, bool)> {
    trace!(count = choices.len(), "preparing skim items");
    let current_profile = std::env::var("AWS_PROFILE").ok();
    let mut roles_per_account: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for choice in choices {
        *roles_per_account
            .entry(choice.account_id.as_str())
            .or_insert(0) += 1;
    }
    let mut lookup = std::collections::HashMap::new();
    let mut items = Vec::with_capacity(choices.len());
    for choice in choices {
        let omit_role_name = roles_per_account
            .get(choice.account_id.as_str())
            .copied()
            .unwrap_or(0)
            == 1;
        let active_profile = aws_config::profile_name_for(choice, omit_role_name);
        // Keep matching legacy profile names to preserve the active marker after upgrades.
        let legacy_profile = aws_config::profile_name_for(choice, false);
        let label = if let Some(profile) = current_profile.as_deref() {
            let prefix = if profile == active_profile || profile == legacy_profile {
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
    if tx.send(items).is_err() {
        return Ok((Vec::new(), false));
    }
    drop(tx);

    let (selected, open_in_browser) = match Skim::run_with(options, Some(rx)) {
        Ok(out) => {
            debug!(is_abort = out.is_abort, "skim run completed");
            if out.is_abort {
                (Vec::new(), false)
            } else {
                let open_in_browser = matches!(
                    &out.final_event,
                    Event::Action(Action::Accept(Some(key))) if key == "ctrl-o"
                );
                (out.selected_items, open_in_browser)
            }
        }
        Err(err) => {
            return Err(Error::Tui(err.to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_options() -> SkimOptions {
        let mut builder = SkimOptionsBuilder::default();
        builder.sync(true);
        builder.build().expect("failed to build skim options")
    }

    #[test]
    fn finds_single_query_match() {
        let options = build_test_options();
        let choices = vec![
            RoleChoice {
                account_id: "111111111111".into(),
                account_name: "Platform".into(),
                role_name: "Admin".into(),
            },
            RoleChoice {
                account_id: "222222222222".into(),
                account_name: "Sandbox".into(),
                role_name: "ReadOnly".into(),
            },
        ];

        let matched = find_single_query_match(&options, &choices, "sandbox");
        assert!(matched.is_some());
        let matched = matched.expect("expected exactly one match");
        assert_eq!(matched.account_name, "Sandbox");
        assert_eq!(matched.role_name, "ReadOnly");
    }

    #[test]
    fn requires_unique_query_match() {
        let options = build_test_options();
        let choices = vec![
            RoleChoice {
                account_id: "111111111111".into(),
                account_name: "Platform".into(),
                role_name: "Admin".into(),
            },
            RoleChoice {
                account_id: "222222222222".into(),
                account_name: "Sandbox".into(),
                role_name: "Admin".into(),
            },
        ];

        let matched = find_single_query_match(&options, &choices, "admin");
        assert!(matched.is_none());
    }

    #[test]
    fn normalizes_initial_query() {
        assert_eq!(normalize_initial_query(None), None);
        assert_eq!(normalize_initial_query(Some("  ")), None);
        assert_eq!(
            normalize_initial_query(Some("  sandbox  ")),
            Some("sandbox".to_string())
        );
    }
}
