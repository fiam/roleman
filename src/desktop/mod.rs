mod detect;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod permissions;
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod unsupported;
mod util;
#[cfg(target_os = "windows")]
mod windows;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PermissionRequirements {
    pub close_auth_browser_tab: bool,
    pub focus_terminal_app: bool,
}

pub(crate) trait Desktop {
    fn close_auth_browser_tab(&self) -> Result<()>;
    fn focus_terminal_app(&self) -> Result<()>;
    fn permission_requirements(&self) -> PermissionRequirements {
        PermissionRequirements::default()
    }
    fn should_warn_close_auth_tab_permission_prompt(&self) -> bool {
        self.permission_requirements().close_auth_browser_tab
    }
    fn close_auth_tab_permission_denied_help(&self, _error: &Error) -> Option<&'static str> {
        None
    }
}

pub fn close_auth_browser_tab() -> Result<()> {
    implementation().close_auth_browser_tab()
}

pub fn focus_terminal_app() -> Result<()> {
    implementation().focus_terminal_app()
}

pub(crate) fn permission_requirements() -> PermissionRequirements {
    implementation().permission_requirements()
}

pub(crate) fn should_warn_close_auth_tab_permission_prompt() -> bool {
    implementation().should_warn_close_auth_tab_permission_prompt()
}

pub(crate) fn close_auth_tab_permission_denied_help(error: &Error) -> Option<&'static str> {
    implementation().close_auth_tab_permission_denied_help(error)
}

#[cfg(target_os = "macos")]
fn implementation() -> &'static dyn Desktop {
    macos::desktop()
}

#[cfg(target_os = "linux")]
fn implementation() -> &'static dyn Desktop {
    linux::desktop()
}

#[cfg(target_os = "windows")]
fn implementation() -> &'static dyn Desktop {
    windows::desktop()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn implementation() -> &'static dyn Desktop {
    unsupported::desktop()
}
