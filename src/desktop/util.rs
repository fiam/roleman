use std::process::Output;

use crate::error::Error;

pub(super) fn command_output_error(program: &str, output: &Output) -> Error {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return Error::Config(format!("{program} failed: {stderr}"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return Error::Config(format!("{program} failed: {stdout}"));
    }
    Error::Config(format!(
        "{program} failed with exit code {}",
        output.status.code().unwrap_or(1)
    ))
}
