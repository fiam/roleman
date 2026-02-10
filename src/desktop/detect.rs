use sysinfo::{Pid, ProcessesToUpdate, System};

#[derive(Debug, Clone, Default)]
pub(crate) struct TerminalTarget {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub(crate) pid: Option<u32>,
    pub(crate) app_name: Option<String>,
}

#[derive(Debug, Clone)]
struct TerminalProcess {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pid: u32,
    app_name: Option<String>,
}

pub(crate) fn detect_terminal_target() -> TerminalTarget {
    let app_override = roleman_terminal_app_override();
    let process = detect_terminal_process_from_parent_chain();
    let app_name = app_override
        .or_else(|| process.as_ref().and_then(|entry| entry.app_name.clone()))
        .or_else(terminal_app_from_term_program);
    TerminalTarget {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        pid: process.map(|entry| entry.pid),
        app_name,
    }
}

fn roleman_terminal_app_override() -> Option<String> {
    let app_name = std::env::var("ROLEMAN_TERMINAL_APP").ok()?;
    let trimmed = app_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

fn detect_terminal_process_from_parent_chain() -> Option<TerminalProcess> {
    use std::collections::HashSet;

    let mut pid = std::process::id();
    let mut seen = HashSet::new();
    let mut shell_parent_pid: Option<u32> = None;
    let mut shell_parent_app_name: Option<String> = None;
    let mut first_parent_pid: Option<u32> = None;
    let mut first_parent_app_name: Option<String> = None;
    for _ in 0..64 {
        if !seen.insert(pid) {
            break;
        }
        let (ppid, command) = match process_snapshot(pid) {
            Some(snapshot) => snapshot,
            None => break,
        };
        if first_parent_pid.is_none() && ppid > 1 {
            first_parent_pid = Some(ppid);
            if let Some((_, parent_command)) = process_snapshot(ppid) {
                first_parent_app_name = app_name_for_command(&parent_command)
                    .or_else(|| guess_gui_app_name_from_command(&parent_command));
            }
        }
        if let Some(app_name) = app_name_for_command(&command) {
            return Some(TerminalProcess {
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                pid,
                app_name: Some(app_name),
            });
        }
        if is_shell_command(&command) && ppid > 1 {
            shell_parent_pid = Some(ppid);
            if shell_parent_app_name.is_none()
                && let Some((_, parent_command)) = process_snapshot(ppid)
            {
                shell_parent_app_name = app_name_for_command(&parent_command)
                    .or_else(|| guess_gui_app_name_from_command(&parent_command));
            }
        }
        if ppid <= 1 {
            break;
        }
        pid = ppid;
    }

    shell_parent_pid
        .map(|pid| {
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            let _ = pid;
            TerminalProcess {
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                pid,
                app_name: shell_parent_app_name,
            }
        })
        .or_else(|| {
            first_parent_pid.map(|pid| {
                #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                let _ = pid;
                TerminalProcess {
                    #[cfg(any(target_os = "linux", target_os = "windows"))]
                    pid,
                    app_name: first_parent_app_name,
                }
            })
        })
}

fn process_snapshot(pid: u32) -> Option<(u32, String)> {
    let mut system = System::new_all();
    let pid = Pid::from_u32(pid);
    system.refresh_processes(ProcessesToUpdate::All, true);
    let process = system.process(pid)?;

    let ppid = process.parent().map(|parent| parent.as_u32()).unwrap_or(0);
    let command = process
        .exe()
        .map(|path| path.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let cmd = process
                .cmd()
                .iter()
                .map(|arg| arg.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" ");
            if cmd.trim().is_empty() {
                None
            } else {
                Some(cmd)
            }
        })
        .or_else(|| {
            let name = process.name().to_string_lossy().to_string();
            if name.trim().is_empty() {
                None
            } else {
                Some(name)
            }
        })?;

    Some((ppid, command))
}

fn terminal_app_for_command(command: &str) -> Option<&'static str> {
    let normalized = command.trim().trim_matches('"');
    if normalized.is_empty() {
        return None;
    }

    let lower = normalized.to_lowercase();
    if lower.contains("visual studio code.app")
        || lower.contains("vscode")
        || lower.ends_with("/code")
        || lower.ends_with("\\code.exe")
    {
        return Some("Visual Studio Code");
    }
    if lower.contains("windows terminal") {
        return Some("Windows Terminal");
    }

    let basename = std::path::Path::new(normalized)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(normalized);
    let base = basename
        .trim_start_matches('-')
        .trim_end_matches(".exe")
        .to_lowercase();

    match base.as_str() {
        "terminal" | "apple_terminal" => Some("Terminal"),
        "iterm" | "iterm2" => Some("iTerm"),
        "warp" | "warpterminal" => Some("Warp"),
        "wezterm" | "wezterm-gui" => Some("WezTerm"),
        "alacritty" => Some("Alacritty"),
        "kitty" => Some("kitty"),
        "hyper" => Some("Hyper"),
        "rio" => Some("Rio"),
        "gnome-terminal" | "gnome-terminal-server" => Some("GNOME Terminal"),
        "konsole" => Some("Konsole"),
        "xfce4-terminal" => Some("Xfce Terminal"),
        "tilix" => Some("Tilix"),
        "terminator" => Some("Terminator"),
        "foot" => Some("foot"),
        "xterm" => Some("xterm"),
        "urxvt" => Some("urxvt"),
        "st" => Some("st"),
        "ptyxis" => Some("Ptyxis"),
        "tabby" => Some("Tabby"),
        "windowsterminal" | "wt" => Some("Windows Terminal"),
        "cmd" | "conhost" => Some("Windows Console Host"),
        "powershell" | "pwsh" => Some("PowerShell"),
        "code" | "code-insiders" => Some("Visual Studio Code"),
        _ => None,
    }
}

fn app_name_for_command(command: &str) -> Option<String> {
    if let Some(name) = terminal_app_for_command(command) {
        return Some(name.to_string());
    }
    app_bundle_name_from_command(command)
}

fn guess_gui_app_name_from_command(command: &str) -> Option<String> {
    let normalized = command.trim().trim_matches('"');
    if normalized.is_empty() {
        return None;
    }

    let basename = std::path::Path::new(normalized)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(normalized);
    let base = basename
        .trim_start_matches('-')
        .trim_end_matches(".exe")
        .trim();
    if base.is_empty() {
        return None;
    }

    let lower = base.to_lowercase();
    if matches!(
        lower.as_str(),
        "roleman"
            | "cargo"
            | "rustc"
            | "bash"
            | "zsh"
            | "fish"
            | "sh"
            | "dash"
            | "ksh"
            | "mksh"
            | "tcsh"
            | "csh"
            | "pwsh"
            | "powershell"
            | "cmd"
            | "login"
            | "launchd"
            | "systemd"
            | "init"
            | "tmux"
            | "screen"
    ) {
        return None;
    }

    Some(title_case_identifier(base))
}

fn app_bundle_name_from_command(command: &str) -> Option<String> {
    let normalized = command.trim().trim_matches('"');
    let marker = ".app/";
    let index = normalized.find(marker)?;
    let prefix = &normalized[..index + 4];
    let bundle_path = std::path::Path::new(prefix);
    let bundle = bundle_path
        .file_stem()?
        .to_string_lossy()
        .trim()
        .to_string();
    if bundle.is_empty() {
        return None;
    }
    Some(match bundle.as_str() {
        "iTerm2" => "iTerm".to_string(),
        other => other.to_string(),
    })
}

fn is_shell_command(command: &str) -> bool {
    let normalized = command.trim().trim_matches('"');
    if normalized.is_empty() {
        return false;
    }

    let basename = std::path::Path::new(normalized)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(normalized);
    let base = basename
        .trim_start_matches('-')
        .trim_end_matches(".exe")
        .to_lowercase();

    matches!(
        base.as_str(),
        "bash"
            | "zsh"
            | "fish"
            | "sh"
            | "dash"
            | "ksh"
            | "mksh"
            | "tcsh"
            | "csh"
            | "nu"
            | "xonsh"
            | "pwsh"
            | "powershell"
            | "cmd"
    )
}

fn terminal_app_from_term_program() -> Option<String> {
    let value = std::env::var("TERM_PROGRAM").ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed {
        "Apple_Terminal" => Some("Terminal".to_string()),
        "iTerm.app" => Some("iTerm".to_string()),
        "WarpTerminal" => Some("Warp".to_string()),
        "WezTerm" => Some("WezTerm".to_string()),
        "vscode" => Some("Visual Studio Code".to_string()),
        "gnome-terminal" => Some("GNOME Terminal".to_string()),
        "konsole" => Some("Konsole".to_string()),
        "xfce4-terminal" => Some("Xfce Terminal".to_string()),
        "Windows_Terminal" => Some("Windows Terminal".to_string()),
        other => Some(title_case_identifier(other)),
    }
}

fn title_case_identifier(value: &str) -> String {
    let parts = value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let first = chars.next().unwrap_or_default();
            let mut out = String::new();
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
            out
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        value.to_string()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        app_bundle_name_from_command, guess_gui_app_name_from_command, is_shell_command,
        process_snapshot, terminal_app_for_command, title_case_identifier,
    };

    #[test]
    fn detects_terminal_from_command_basename() {
        assert_eq!(
            terminal_app_for_command("/Applications/iTerm.app/Contents/MacOS/iTerm2"),
            Some("iTerm")
        );
        assert_eq!(
            terminal_app_for_command("/Applications/Warp.app/Contents/MacOS/Warp"),
            Some("Warp")
        );
        assert_eq!(
            terminal_app_for_command("/usr/bin/gnome-terminal-server"),
            Some("GNOME Terminal")
        );
        assert_eq!(terminal_app_for_command("wezterm-gui"), Some("WezTerm"));
        assert_eq!(terminal_app_for_command("-zsh"), None);
    }

    #[test]
    fn detects_vscode_from_app_path() {
        assert_eq!(
            terminal_app_for_command(
                "/Applications/Visual Studio Code.app/Contents/MacOS/Electron"
            ),
            Some("Visual Studio Code")
        );
    }

    #[test]
    fn extracts_app_name_from_bundle_path() {
        assert_eq!(
            app_bundle_name_from_command("/Applications/Ghostty.app/Contents/MacOS/ghostty"),
            Some("Ghostty".to_string())
        );
        assert_eq!(
            app_bundle_name_from_command("/Applications/iTerm2.app/Contents/MacOS/iTerm2"),
            Some("iTerm".to_string())
        );
    }

    #[test]
    fn detects_shell_commands() {
        assert!(is_shell_command("/bin/zsh"));
        assert!(is_shell_command("-bash"));
        assert!(is_shell_command("pwsh.exe"));
        assert!(!is_shell_command(
            "/Applications/Terminal.app/Contents/MacOS/Terminal"
        ));
    }

    #[test]
    fn guesses_gui_app_name_without_whitelist() {
        assert_eq!(
            guess_gui_app_name_from_command("/Applications/Ghostty.app/Contents/MacOS/ghostty"),
            Some("Ghostty".to_string())
        );
        assert_eq!(
            guess_gui_app_name_from_command("/opt/homebrew/bin/ghostty"),
            Some("Ghostty".to_string())
        );
        assert_eq!(guess_gui_app_name_from_command("/bin/zsh"), None);
        assert_eq!(guess_gui_app_name_from_command("launchd"), None);
    }

    #[test]
    fn title_cases_identifiers() {
        assert_eq!(title_case_identifier("ghostty"), "Ghostty");
        assert_eq!(
            title_case_identifier("windows_terminal"),
            "Windows Terminal"
        );
        assert_eq!(title_case_identifier("wezterm"), "Wezterm");
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    #[test]
    fn reads_snapshot_for_current_process() {
        let snapshot = process_snapshot(std::process::id());
        assert!(snapshot.is_some());
    }
}
