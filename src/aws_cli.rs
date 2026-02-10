use std::process::{Command, Stdio};

use crate::aws_config;
use crate::desktop;
use crate::error::{Error, Result};
use crate::ui;

#[derive(Debug, Clone, Copy, Default)]
pub struct PostLoginActions {
    pub focus_terminal: bool,
    pub close_browser_tab: bool,
}

pub fn sso_login_session(session: &str, post_login_actions: PostLoginActions) -> Result<()> {
    ui::print_info(&format!(
        "Running `aws sso login --sso-session {}`...",
        session
    ));
    let config_path = aws_config::aws_config_path()?;
    let status = Command::new("aws")
        .arg("sso")
        .arg("login")
        .arg("--sso-session")
        .arg(session)
        .env("AWS_CONFIG_FILE", config_path)
        .env_remove("AWS_PROFILE")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                return Error::Config(
                    "aws CLI not found in PATH. Install AWS CLI v2 and ensure `aws` is available."
                        .to_string(),
                );
            }
            Error::Config(format!("failed to run aws cli (v2 required): {err}"))
        })?;

    if status.success() {
        run_post_login_actions(post_login_actions);
        return Ok(());
    }

    let code = status.code().unwrap_or(1);
    Err(Error::Config(format!(
        "aws sso login failed with exit code {code}"
    )))
}

fn run_post_login_actions(actions: PostLoginActions) {
    if !actions.focus_terminal && !actions.close_browser_tab {
        return;
    }

    let permission_requirements = desktop::permission_requirements();

    if actions.close_browser_tab {
        if permission_requirements.close_auth_browser_tab
            && desktop::should_warn_close_auth_tab_permission_prompt()
        {
            ui::print_warn(
                "Closing the auth tab may require OS automation permission on this platform.",
            );
        }
        if let Err(err) = desktop::close_auth_browser_tab() {
            if let Some(help) = desktop::close_auth_tab_permission_denied_help(&err) {
                ui::print_warn(help);
            }
            ui::print_warn(&format!("Post-auth automation skipped: {err}"));
            tracing::debug!(error = %err, "post-auth close browser tab failed");
        }
    }

    if actions.focus_terminal
        && let Err(err) = desktop::focus_terminal_app()
    {
        if permission_requirements.focus_terminal_app {
            tracing::debug!("focus-terminal action may require OS permission on this platform");
        }
        ui::print_warn(&format!("Post-auth automation skipped: {err}"));
        tracing::debug!(error = %err, "post-auth focus terminal failed");
    }
}
