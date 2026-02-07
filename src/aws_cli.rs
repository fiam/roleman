use std::process::{Command, Stdio};

use crate::aws_config;
use crate::error::{Error, Result};
use crate::ui;

pub fn sso_login_session(session: &str) -> Result<()> {
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
        return Ok(());
    }

    let code = status.code().unwrap_or(1);
    Err(Error::Config(format!(
        "aws sso login failed with exit code {code}"
    )))
}
