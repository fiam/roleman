use std::borrow::Cow;
use std::sync::Arc;

use skim::prelude::*;
use tracing::{debug, trace};

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

pub fn select_role(prompt: &str, choices: &[RoleChoice]) -> Result<Option<TuiSelection>> {
    if choices.is_empty() {
        return Ok(None);
    }

    let mut ordered = choices.to_vec();
    ordered.reverse();
    debug!(count = ordered.len(), "starting role selection");
    let options = SkimOptionsBuilder::default()
        .height(Some("50%"))
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

    let (selected, open_in_browser) = run_skim(&options, &ordered)?;

    if selected.is_empty() {
        debug!("no role selected");
        return Ok(None);
    }

    Ok(Some(TuiSelection {
        choice: selected[0].clone(),
        open_in_browser,
    }))
}

fn run_skim(options: &SkimOptions, choices: &[RoleChoice]) -> Result<(Vec<RoleChoice>, bool)> {
    trace!(count = choices.len(), "preparing skim items");
    let mut lookup = std::collections::HashMap::new();
    let items = choices
        .iter()
        .map(|choice| {
            let label = choice.label();
            lookup.insert(label.clone(), choice.clone());
            ChoiceItem { label }
        })
        .map(|item| Arc::new(item) as Arc<dyn SkimItem>)
        .collect::<Vec<_>>();

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
