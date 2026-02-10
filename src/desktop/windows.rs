use std::process::Command;

use crate::error::{Error, Result};

use super::Desktop;
use super::detect::detect_terminal_target;
use super::util::command_output_error;

pub(super) struct WindowsDesktop;

static DESKTOP: WindowsDesktop = WindowsDesktop;

pub(super) fn desktop() -> &'static dyn Desktop {
    &DESKTOP
}

impl Desktop for WindowsDesktop {
    fn close_auth_browser_tab(&self) -> Result<()> {
        run_powershell_status(
            r#"Add-Type -Namespace RolemanWin -Name User32 -MemberDefinition '[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow(); [System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder text, int count);' -ErrorAction SilentlyContinue | Out-Null; $h = [RolemanWin.User32]::GetForegroundWindow(); if ($h -eq [IntPtr]::Zero) { return }; $sb = New-Object System.Text.StringBuilder 2048; [void][RolemanWin.User32]::GetWindowText($h, $sb, $sb.Capacity); $title = $sb.ToString().ToLowerInvariant(); if ($title -match '127\.0\.0\.1|localhost') { $wshell = New-Object -ComObject WScript.Shell; $wshell.SendKeys('^w') }"#,
        )
    }

    fn focus_terminal_app(&self) -> Result<()> {
        let target = detect_terminal_target();
        if let Some(pid) = target.pid
            && activate_window_for_pid(pid).is_ok()
        {
            return Ok(());
        }

        if let Some(app_name) = target.app_name
            && let Some(process_name) = windows_process_name_for_app(&app_name)
            && activate_window_for_process_name(process_name).is_ok()
        {
            return Ok(());
        }

        Err(Error::Config(
            "could not focus terminal window on Windows. Set ROLEMAN_TERMINAL_APP to the terminal app name and run from that terminal.".to_string(),
        ))
    }
}

fn activate_window_for_pid(pid: u32) -> Result<()> {
    let script = format!(
        "$p = Get-Process -Id {pid} -ErrorAction Stop; if ($p.MainWindowHandle -eq 0) {{ throw 'process has no main window' }}; Add-Type -Namespace RolemanWin -Name User32 -MemberDefinition '[System.Runtime.InteropServices.DllImport(\"user32.dll\")] public static extern bool SetForegroundWindow(System.IntPtr hWnd);' -ErrorAction SilentlyContinue | Out-Null; [void][RolemanWin.User32]::SetForegroundWindow($p.MainWindowHandle)"
    );
    run_powershell_status(&script)
}

fn activate_window_for_process_name(process_name: &str) -> Result<()> {
    let process_name = powershell_single_quote(process_name);
    let script = format!(
        "$p = Get-Process -Name '{process_name}' -ErrorAction Stop | Where-Object {{ $_.MainWindowHandle -ne 0 }} | Select-Object -First 1; if ($null -eq $p) {{ throw 'process has no main window' }}; Add-Type -Namespace RolemanWin -Name User32 -MemberDefinition '[System.Runtime.InteropServices.DllImport(\"user32.dll\")] public static extern bool SetForegroundWindow(System.IntPtr hWnd);' -ErrorAction SilentlyContinue | Out-Null; [void][RolemanWin.User32]::SetForegroundWindow($p.MainWindowHandle)"
    );
    run_powershell_status(&script)
}

fn windows_process_name_for_app(app_name: &str) -> Option<&'static str> {
    match app_name {
        "Windows Terminal" => Some("WindowsTerminal"),
        "PowerShell" => Some("pwsh"),
        "Visual Studio Code" => Some("Code"),
        "WezTerm" => Some("wezterm-gui"),
        "Alacritty" => Some("alacritty"),
        "Warp" => Some("warp"),
        _ => None,
    }
}

fn run_powershell_status(script: &str) -> Result<()> {
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|err| Error::Config(format!("failed to run powershell: {err}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(command_output_error("powershell", &output))
}

fn powershell_single_quote(value: &str) -> String {
    value.replace('\'', "''")
}
