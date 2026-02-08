use std::path::{Path, PathBuf};

use super::Shell;

#[derive(Clone, Copy, Debug)]
pub struct BashShell;

pub static BASH_SHELL: BashShell = BashShell;

impl Shell for BashShell {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn hook_snippet(&self) -> &'static str {
        r##"export _ROLEMAN_HOOK_ENV="${XDG_STATE_HOME:-$HOME/.local/state}/roleman/env-${TTY//\//_}"
export _ROLEMAN_HOOK_VERSION=1
roleman() {
  command roleman --env-file "$_ROLEMAN_HOOK_ENV" "$@"
}
_roleman_prompt_command() {
  if [[ -f "$_ROLEMAN_HOOK_ENV" ]]; then
    source "$_ROLEMAN_HOOK_ENV"
    rm -f "$_ROLEMAN_HOOK_ENV"
  fi
}
if [[ -n "${PROMPT_COMMAND:-}" ]]; then
  PROMPT_COMMAND="_roleman_prompt_command;${PROMPT_COMMAND}"
else
  PROMPT_COMMAND="_roleman_prompt_command"
fi"##
    }

    fn rc_path(&self) -> Result<PathBuf, String> {
        let home = std::env::var("HOME").map_err(|_| "missing HOME".to_string())?;
        Ok(Path::new(&home).join(".bashrc"))
    }
}
