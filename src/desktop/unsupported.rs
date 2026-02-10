use crate::error::{Error, Result};

use super::Desktop;

pub(super) struct UnsupportedDesktop;

static DESKTOP: UnsupportedDesktop = UnsupportedDesktop;

pub(super) fn desktop() -> &'static dyn Desktop {
    &DESKTOP
}

impl Desktop for UnsupportedDesktop {
    fn close_auth_browser_tab(&self) -> Result<()> {
        unsupported()
    }

    fn focus_terminal_app(&self) -> Result<()> {
        unsupported()
    }
}

fn unsupported() -> Result<()> {
    Err(Error::Config(
        "post-auth automation is not supported on this operating system".to_string(),
    ))
}
