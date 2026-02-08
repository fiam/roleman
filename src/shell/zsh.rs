use std::path::{Path, PathBuf};

use super::Shell;

#[derive(Clone, Copy, Debug)]
pub struct ZshShell;

pub static ZSH_SHELL: ZshShell = ZshShell;

impl Shell for ZshShell {
    fn name(&self) -> &'static str {
        "zsh"
    }

    fn hook_snippet(&self) -> &'static str {
        r##"export _ROLEMAN_HOOK_ENV="${XDG_STATE_HOME:-$HOME/.local/state}/roleman/env-${TTY//\//_}"
export _ROLEMAN_HOOK_VERSION=1
roleman() {
  command roleman --env-file "$_ROLEMAN_HOOK_ENV" "$@"
}
_roleman_precmd() {
  if [[ -f "$_ROLEMAN_HOOK_ENV" ]]; then
    source "$_ROLEMAN_HOOK_ENV"
    rm -f "$_ROLEMAN_HOOK_ENV"
  fi
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd _roleman_precmd"##
    }

    fn rc_path(&self) -> Result<PathBuf, String> {
        let home = std::env::var("HOME").map_err(|_| "missing HOME".to_string())?;
        Ok(Path::new(&home).join(".zshrc"))
    }
}
