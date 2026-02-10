use std::process::{Command, Output};

use crate::error::{Error, Result};

use super::Desktop;
use super::detect::detect_terminal_target;
use super::permissions::{macos_close_auth_tab_authorized, set_macos_close_auth_tab_authorized};
use super::util::command_output_error;

const MAC_AUTOMATION_PERMISSION_DENIED_ERROR: &str =
    "macOS automation permission denied for close-auth-tab";
const MAC_AUTOMATION_PERMISSION_HELP: &str = "When prompted, click Allow. If you previously denied it, open System Settings > Privacy & Security > Automation and allow your terminal app to control System Events and your browser.";

pub(super) struct MacDesktop;

static DESKTOP: MacDesktop = MacDesktop;

pub(super) fn desktop() -> &'static dyn Desktop {
    &DESKTOP
}

impl Desktop for MacDesktop {
    fn close_auth_browser_tab(&self) -> Result<()> {
        let tab_url = match frontmost_browser_tab_url() {
            Ok(tab_url) => tab_url,
            Err(err) => {
                if is_macos_automation_permission_denied(&err) {
                    set_macos_close_auth_tab_authorized(false);
                }
                return Err(err);
            }
        };

        if tab_url.is_some() {
            set_macos_close_auth_tab_authorized(true);
        }

        if tab_url.as_deref().is_some_and(is_loopback_auth_url) {
            if let Err(err) = close_front_tab() {
                if is_macos_automation_permission_denied(&err) {
                    set_macos_close_auth_tab_authorized(false);
                }
                return Err(err);
            }
            set_macos_close_auth_tab_authorized(true);
        }

        Ok(())
    }

    fn focus_terminal_app(&self) -> Result<()> {
        let target = detect_terminal_target();
        if let Some(app_name) = target.app_name.as_deref()
            && activate_app_with_open(app_name).is_ok()
        {
            return Ok(());
        }

        Err(Error::Config(
            "could not detect terminal app. Set ROLEMAN_TERMINAL_APP to your app name (for example: Terminal, iTerm, Warp, WezTerm).".to_string(),
        ))
    }

    fn permission_requirements(&self) -> super::PermissionRequirements {
        super::PermissionRequirements {
            close_auth_browser_tab: true,
            focus_terminal_app: false,
        }
    }

    fn should_warn_close_auth_tab_permission_prompt(&self) -> bool {
        !macos_close_auth_tab_authorized()
    }

    fn close_auth_tab_permission_denied_help(&self, error: &Error) -> Option<&'static str> {
        if is_macos_automation_permission_denied(error) {
            return Some(MAC_AUTOMATION_PERMISSION_HELP);
        }
        None
    }
}

fn activate_app_with_open(app_name: &str) -> Result<()> {
    let output = Command::new("open")
        .args(["-a", app_name])
        .output()
        .map_err(|err| Error::Config(format!("failed to run open: {err}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(command_output_error("open", &output))
}

fn frontmost_browser_tab_url() -> Result<Option<String>> {
    let output = run_osascript_capture([
        r#"tell application "System Events" to set frontApp to name of first process whose frontmost is true"#,
        r#"set tabUrl to """#,
        r#"set chromiumApps to {"Google Chrome", "Brave Browser", "Arc", "Microsoft Edge"}"#,
        r#"if frontApp is "Safari" then"#,
        r#"    tell application "Safari""#,
        r#"        if (count of windows) > 0 then"#,
        r#"            set frontWindow to front window"#,
        r#"            if (count of tabs of frontWindow) > 0 then set tabUrl to URL of current tab of frontWindow"#,
        r#"        end if"#,
        r#"    end tell"#,
        r#"else if chromiumApps contains frontApp then"#,
        r#"    using terms from application "Google Chrome""#,
        r#"        tell application frontApp"#,
        r#"            if (count of windows) > 0 then"#,
        r#"                set frontWindow to front window"#,
        r#"                if (count of tabs of frontWindow) > 0 then set tabUrl to URL of active tab of frontWindow"#,
        r#"            end if"#,
        r#"        end tell"#,
        r#"    end using terms from"#,
        r#"else if frontApp is "Firefox" then"#,
        r#"    try"#,
        r#"        tell application "Firefox" to set tabUrl to URL of front document"#,
        r#"    end try"#,
        r#"end if"#,
        r#"return tabUrl"#,
    ])?;

    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(trimmed.to_string()))
}

fn close_front_tab() -> Result<()> {
    run_osascript([r#"tell application "System Events" to keystroke "w" using command down"#])
}

fn is_loopback_auth_url(url: &str) -> bool {
    let Some(host) = url_host(url) else {
        return false;
    };

    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

fn url_host(url: &str) -> Option<&str> {
    let (_, remainder) = url.trim().split_once("://")?;
    let authority = remainder.split('/').next().unwrap_or(remainder);
    let authority = authority.rsplit('@').next().unwrap_or(authority);

    if let Some(stripped) = authority.strip_prefix('[') {
        let end = stripped.find(']')?;
        return Some(&stripped[..end]);
    }

    Some(authority.split(':').next().unwrap_or(authority))
}

fn run_osascript<I, S>(lines: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let output = run_osascript_output(lines)?;
    if output.status.success() {
        return Ok(());
    }
    if osascript_permission_denied(&output) {
        return Err(permission_denied_error());
    }
    Err(command_output_error("osascript", &output))
}

fn run_osascript_capture<I, S>(lines: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let output = run_osascript_output(lines)?;
    if !output.status.success() {
        if osascript_permission_denied(&output) {
            return Err(permission_denied_error());
        }
        return Err(command_output_error("osascript", &output));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_osascript_output<I, S>(lines: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut command = Command::new("osascript");
    for line in lines {
        command.arg("-e").arg(line.as_ref());
    }
    command
        .output()
        .map_err(|err| Error::Config(format!("failed to run osascript: {err}")))
}

fn osascript_permission_denied(output: &Output) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}\n{stdout}");
    let lower = combined.to_lowercase();
    lower.contains("(-1743)")
        || lower.contains("not authorized to send apple events")
        || lower.contains("erraeeventnotpermitted")
        || lower.contains("not allowed assistive access")
        || (lower.contains("accessibility") && lower.contains("not allowed"))
}

fn permission_denied_error() -> Error {
    Error::Config(MAC_AUTOMATION_PERMISSION_DENIED_ERROR.to_string())
}

fn is_macos_automation_permission_denied(err: &Error) -> bool {
    matches!(err, Error::Config(message) if message == MAC_AUTOMATION_PERMISSION_DENIED_ERROR)
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Output;

    use super::{is_loopback_auth_url, osascript_permission_denied, url_host};

    #[test]
    fn parses_url_host() {
        assert_eq!(
            url_host("http://127.0.0.1:52391/callback"),
            Some("127.0.0.1")
        );
        assert_eq!(url_host("https://localhost/path"), Some("localhost"));
        assert_eq!(url_host("https://[::1]:3000/path"), Some("::1"));
        assert_eq!(url_host("not-a-url"), None);
    }

    #[test]
    fn matches_loopback_auth_urls() {
        assert!(is_loopback_auth_url("http://127.0.0.1:52391/callback"));
        assert!(is_loopback_auth_url("https://localhost:52391/callback"));
        assert!(is_loopback_auth_url("http://[::1]:52391/callback"));
        assert!(!is_loopback_auth_url("https://example.com/callback"));
        assert!(!is_loopback_auth_url(
            "https://localhost.evil.example/callback"
        ));
    }

    #[test]
    fn detects_osascript_permission_denied() {
        let output = Output {
            status: std::process::ExitStatus::from_raw(1),
            stdout: Vec::new(),
            stderr:
                b"execution error: Not authorized to send Apple events to System Events. (-1743)"
                    .to_vec(),
        };
        assert!(osascript_permission_denied(&output));
    }

    #[test]
    fn ignores_unrelated_osascript_errors() {
        let output = Output {
            status: std::process::ExitStatus::from_raw(1),
            stdout: Vec::new(),
            stderr: b"execution error: Variable is not defined. (-2753)".to_vec(),
        };
        assert!(!osascript_permission_denied(&output));
    }
}
