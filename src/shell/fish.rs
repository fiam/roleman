use std::path::{Path, PathBuf};

use super::Shell;

#[derive(Clone, Copy, Debug)]
pub struct FishShell;

pub static FISH_SHELL: FishShell = FishShell;

impl Shell for FishShell {
    fn name(&self) -> &'static str {
        "fish"
    }

    fn hook_snippet(&self) -> &'static str {
        r##"if set -q XDG_STATE_HOME
  set -gx _ROLEMAN_HOOK_ENV "$XDG_STATE_HOME/roleman/env-(string replace -a '/' '_' (tty))"
else
  set -gx _ROLEMAN_HOOK_ENV "$HOME/.local/state/roleman/env-(string replace -a '/' '_' (tty))"
end
set -gx _ROLEMAN_HOOK_VERSION 1
function roleman
  command roleman --env-file "$_ROLEMAN_HOOK_ENV" $argv
end
function __roleman_prompt --on-event fish_prompt
  if test -f "$_ROLEMAN_HOOK_ENV"
    source "$_ROLEMAN_HOOK_ENV"
    rm -f "$_ROLEMAN_HOOK_ENV"
  end
end"##
    }

    fn rc_path(&self) -> Result<PathBuf, String> {
        if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME")
            && !config_home.is_empty()
        {
            return Ok(Path::new(&config_home).join("fish").join("config.fish"));
        }
        let home = std::env::var("HOME").map_err(|_| "missing HOME".to_string())?;
        Ok(Path::new(&home)
            .join(".config")
            .join("fish")
            .join("config.fish"))
    }

    fn install_line(&self) -> String {
        "roleman hook fish | source".to_string()
    }

    fn alias_line(&self) -> &'static str {
        "alias rl roleman"
    }
}
