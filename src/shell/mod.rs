use std::path::{Path, PathBuf};

mod bash;
mod fish;
mod zsh;

use bash::BASH_SHELL;
use fish::FISH_SHELL;
use zsh::ZSH_SHELL;

pub trait Shell {
    fn name(&self) -> &'static str;
    fn hook_snippet(&self) -> &'static str;
    fn rc_path(&self) -> Result<PathBuf, String>;

    fn install_line(&self) -> String {
        format!("eval \"$(roleman hook {})\"", self.name())
    }

    fn alias_line(&self) -> &'static str {
        "alias rl='roleman'"
    }

    fn reload_command(&self, rc_path: &Path) -> String {
        format!("source {}", rc_path.display())
    }
}

pub fn shell_for_name(name: &str) -> Option<&'static dyn Shell> {
    match name {
        "zsh" => Some(&ZSH_SHELL),
        "bash" => Some(&BASH_SHELL),
        "fish" => Some(&FISH_SHELL),
        _ => None,
    }
}

pub fn detect_shell_from_env() -> Option<&'static dyn Shell> {
    let shell = std::env::var("SHELL").ok()?;
    let name = Path::new(&shell).file_name()?.to_str()?;
    shell_for_name(name)
}

#[cfg(test)]
mod tests {
    use super::shell_for_name;

    #[test]
    fn resolves_supported_shells() {
        assert!(shell_for_name("bash").is_some());
        assert!(shell_for_name("zsh").is_some());
        assert!(shell_for_name("fish").is_some());
    }

    #[test]
    fn fish_uses_fish_specific_install_line() {
        let fish = shell_for_name("fish").expect("fish shell should be supported");
        assert_eq!(fish.install_line(), "roleman hook fish | source");
    }
}
