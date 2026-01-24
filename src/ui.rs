use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;

pub fn spinner(message: &str) -> ProgressBar {
    let style = ProgressStyle::with_template("{spinner} {msg}")
        .unwrap()
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]);
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(style);
    spinner.set_message(message.to_string());
    spinner.enable_steady_tick(Duration::from_millis(80));
    spinner
}

pub fn success(message: &str) -> String {
    format!("✅ {}", message.green())
}

pub fn info(message: &str) -> String {
    format!("ℹ️ {}", message.blue())
}

pub fn warn(message: &str) -> String {
    format!("⚠️ {}", message.yellow())
}

pub fn action(message: &str) -> String {
    format!("🔍 {}", message.cyan())
}

pub fn hint(message: &str) -> String {
    format!("› {}", message.dimmed())
}
