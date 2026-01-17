use std::borrow::Cow;
use std::sync::Arc;

use skim::prelude::*;
use tracing::{debug, trace};

use crate::config::HiddenRole;
use crate::error::{Error, Result};
use crate::model::RoleChoice;

struct ChoiceItem {
    label: String,
}

impl SkimItem for ChoiceItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.label)
    }
}

pub fn select_role(choices: &[RoleChoice]) -> Result<Option<RoleChoice>> {
    if choices.is_empty() {
        return Ok(None);
    }

    debug!(count = choices.len(), "starting role selection");
    let options = SkimOptionsBuilder::default()
        .height(Some("50%"))
        .multi(false)
        .prompt(Some("roleman> "))
        .build()
        .map_err(|err| Error::Tui(err.to_string()))?;

    let selected = run_skim(&options, choices)?;

    if selected.is_empty() {
        debug!("no role selected");
        return Ok(None);
    }

    Ok(Some(selected[0].clone()))
}

pub fn manage_hidden(
    choices: &[RoleChoice],
    hidden: &[HiddenRole],
) -> Result<Vec<HiddenRole>> {
    let mut hidden_set: std::collections::HashSet<HiddenRole> =
        hidden.iter().cloned().collect();

    let hidden_choices = choices
        .iter()
        .filter(|choice| hidden_set.iter().any(|entry| entry.matches(choice)))
        .cloned()
        .collect::<Vec<_>>();
    if !hidden_choices.is_empty() {
        let options = SkimOptionsBuilder::default()
            .height(Some("50%"))
            .multi(true)
            .prompt(Some("unhide> "))
            .build()
            .map_err(|err| Error::Tui(err.to_string()))?;
        let selected = run_skim(&options, &hidden_choices)?;
        for item in selected {
            hidden_set.remove(&HiddenRole::from_choice(&item));
        }
    }

    let visible_choices = choices
        .iter()
        .filter(|choice| !hidden_set.iter().any(|entry| entry.matches(choice)))
        .cloned()
        .collect::<Vec<_>>();
    if !visible_choices.is_empty() {
        let options = SkimOptionsBuilder::default()
            .height(Some("50%"))
            .multi(true)
            .prompt(Some("hide> "))
            .build()
            .map_err(|err| Error::Tui(err.to_string()))?;
        let selected = run_skim(&options, &visible_choices)?;
        for item in selected {
            hidden_set.insert(HiddenRole::from_choice(&item));
        }
    }

    let mut updated: Vec<HiddenRole> = hidden_set.into_iter().collect();
    updated.sort_by(|a, b| a.account_id.cmp(&b.account_id).then(a.role_name.cmp(&b.role_name)));
    Ok(updated)
}

fn run_skim(options: &SkimOptions, choices: &[RoleChoice]) -> Result<Vec<RoleChoice>> {
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

    let selected = match Skim::run_with(options, Some(rx)) {
        Some(out) => {
            debug!(is_abort = out.is_abort, "skim run completed");
            out.selected_items
        }
        None => {
            debug!("skim returned no output");
            Vec::new()
        }
    };
    debug!(count = selected.len(), "skim selection complete");

    let mut result = Vec::new();
    for item in selected {
        let key = item.text();
        if let Some(choice) = lookup.get(key.as_ref()) {
            result.push(choice.clone());
        } else {
            debug!(value = %key, "missing selection lookup");
        }
    }
    Ok(result)
}
