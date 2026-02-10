use std::process::Command;

use crate::error::{Error, Result};

use super::Desktop;
use super::detect::detect_terminal_target;
use super::util::command_output_error;

pub(super) struct LinuxDesktop;

static DESKTOP: LinuxDesktop = LinuxDesktop;

pub(super) fn desktop() -> &'static dyn Desktop {
    &DESKTOP
}

impl Desktop for LinuxDesktop {
    fn close_auth_browser_tab(&self) -> Result<()> {
        let title = active_window_title()?;
        if !title_mentions_loopback(&title) {
            tracing::debug!(window_title = %title, "skipping auth tab close because active window title is not loopback");
            return Ok(());
        }

        let output = Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+w"])
            .output()
            .map_err(|err| Error::Config(format!("failed to run xdotool: {err}")))?;
        if output.status.success() {
            return Ok(());
        }
        Err(command_output_error("xdotool", &output))
    }

    fn focus_terminal_app(&self) -> Result<()> {
        let target = detect_terminal_target();
        if let Some(pid) = target.pid
            && activate_window_for_pid(pid).is_ok()
        {
            return Ok(());
        }

        if let Some(app_name) = target.app_name {
            let pattern = linux_window_pattern_for_app(&app_name);
            if activate_window_for_app(&pattern).is_ok() {
                return Ok(());
            }
        }

        Err(Error::Config(
            "could not focus terminal window on Linux. Install `xdotool` (preferred) or `wmctrl`, or set ROLEMAN_TERMINAL_APP.".to_string(),
        ))
    }
}

fn activate_window_for_pid(pid: u32) -> Result<()> {
    let output = Command::new("xdotool")
        .args([
            "search",
            "--onlyvisible",
            "--pid",
            &pid.to_string(),
            "windowactivate",
        ])
        .output()
        .map_err(|err| Error::Config(format!("failed to run xdotool: {err}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(command_output_error("xdotool", &output))
}

fn activate_window_for_app(app_pattern: &str) -> Result<()> {
    if let Ok(output) = Command::new("xdotool")
        .args([
            "search",
            "--onlyvisible",
            "--name",
            app_pattern,
            "windowactivate",
        ])
        .output()
        && output.status.success()
    {
        return Ok(());
    }

    let output = Command::new("wmctrl")
        .args(["-xa", app_pattern])
        .output()
        .map_err(|err| Error::Config(format!("failed to run wmctrl: {err}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(command_output_error("wmctrl", &output))
}

fn linux_window_pattern_for_app(app_name: &str) -> String {
    match app_name {
        "GNOME Terminal" => "gnome-terminal".to_string(),
        "Konsole" => "konsole".to_string(),
        "Xfce Terminal" => "xfce4-terminal".to_string(),
        "Visual Studio Code" => "code".to_string(),
        other => other.to_string(),
    }
}

fn active_window_title() -> Result<String> {
    let output = Command::new("xdotool")
        .args(["getactivewindow", "getwindowname"])
        .output()
        .map_err(|err| Error::Config(format!("failed to run xdotool: {err}")))?;
    if !output.status.success() {
        return Err(command_output_error("xdotool", &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn title_mentions_loopback(title: &str) -> bool {
    let lower = title.to_lowercase();
    lower.contains("127.0.0.1") || lower.contains("localhost")
}
