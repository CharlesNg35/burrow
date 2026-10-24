use std::path::Path;
use std::process::Command;

use crate::base::failure::Failure;

const FALLBACKS: &[&str] = if cfg!(windows) {
    &["notepad"]
} else {
    &["nano", "vim", "vi"]
};

pub fn choose(
    given: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
    installed: impl Fn(&str) -> bool,
) -> Result<Vec<String>, Failure> {
    let named = given
        .map(str::to_string)
        .or_else(|| env("SUDO_EDITOR"))
        .or_else(|| env("VISUAL"))
        .or_else(|| env("EDITOR"))
        .filter(|c| !c.trim().is_empty());
    if let Some(command) = named {
        let words = split(&command);
        if words.is_empty() {
            return Err(Failure::usage(format!(
                "cannot read the editor command {command:?}"
            )));
        }
        return Ok(words);
    }
    FALLBACKS
        .iter()
        .find(|e| installed(e))
        .map(|e| vec![e.to_string()])
        .ok_or_else(|| {
            Failure::usage("no editor found").hint(
                "set one with --editor, or the VISUAL or EDITOR environment variable, for example: --editor nano",
            )
        })
}

pub fn split(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in command.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

pub fn on_path(program: &str) -> bool {
    which::which(program).is_ok()
}

pub fn open(editor: &[String], file: &Path) -> Result<(), Failure> {
    let (program, args) = editor
        .split_first()
        .expect("an editor command has a program");
    let resolved = which::which(program).map_err(|e| {
        Failure::usage(format!("cannot start the editor {program}: {e}"))
            .hint("pick another with --editor, for example: --editor nano")
    })?;
    let status = Command::new(resolved)
        .args(args)
        .arg(file)
        .status()
        .map_err(|e| {
            Failure::usage(format!("cannot start the editor {program}: {e}"))
                .hint("pick another with --editor, for example: --editor nano")
        })?;
    if !status.success() {
        return Err(Failure::new(format!(
            "the editor {program} exited with {status}; nothing was changed"
        )));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) struct Quiet {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    outside: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(unix)]
impl Quiet {
    pub(super) fn new() -> Quiet {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        static FLAGS: std::sync::OnceLock<(Arc<AtomicBool>, Arc<AtomicBool>)> =
            std::sync::OnceLock::new();
        let (stop, outside) = FLAGS.get_or_init(|| {
            let stop = Arc::new(AtomicBool::new(false));
            let outside = Arc::new(AtomicBool::new(true));
            for sig in [SIGINT, SIGQUIT, SIGTERM, SIGHUP] {
                let _ = signal_hook::flag::register_conditional_default(sig, outside.clone());
            }
            for sig in [SIGTERM, SIGHUP] {
                let _ = signal_hook::flag::register(sig, stop.clone());
            }
            (stop, outside)
        });
        stop.store(false, Ordering::SeqCst);
        outside.store(false, Ordering::SeqCst);
        Quiet {
            stop: stop.clone(),
            outside: outside.clone(),
        }
    }

    pub(super) fn stopped(&self) -> bool {
        self.stop.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(unix)]
impl Drop for Quiet {
    fn drop(&mut self) {
        self.outside
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(windows)]
pub(super) struct Quiet {
    _keys: Option<(
        tokio::runtime::Runtime,
        tokio::signal::windows::CtrlC,
        tokio::signal::windows::CtrlBreak,
    )>,
}

#[cfg(windows)]
impl Quiet {
    pub(super) fn new() -> Quiet {
        let keys = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()
            .and_then(|rt| {
                let (c, b) = rt.block_on(async {
                    (
                        tokio::signal::windows::ctrl_c(),
                        tokio::signal::windows::ctrl_break(),
                    )
                });
                Some((rt, c.ok()?, b.ok()?))
            });
        Quiet { _keys: keys }
    }

    pub(super) fn stopped(&self) -> bool {
        false
    }
}

#[cfg(not(any(unix, windows)))]
pub(super) struct Quiet;

#[cfg(not(any(unix, windows)))]
impl Quiet {
    pub(super) fn new() -> Quiet {
        Quiet
    }

    pub(super) fn stopped(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_editor_is_resolved_before_it_runs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let editor = dir.path().join("fake-editor");
        std::fs::write(&editor, "#!/bin/sh\necho edited > \"$1\"\n").unwrap();
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = dir.path().join("server.yaml");
        std::fs::write(&file, "x").unwrap();
        open(&[editor.display().to_string()], &file).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "edited\n");
        let missing = open(&["burrow-no-such-editor".into()], &file).unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("cannot start the editor burrow-no-such-editor")
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_cmd_editor_is_found_through_pathext() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("code.cmd"), "@echo edited> %1\r\n").unwrap();
        let file = dir.path().join("server.yaml");
        std::fs::write(&file, "x").unwrap();
        let editor = dir.path().join("code");
        open(&[editor.display().to_string()], &file).unwrap();
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .starts_with("edited")
        );
    }

    #[test]
    fn editor_commands_keep_their_arguments_and_quoted_paths() {
        assert_eq!(split("code --wait"), vec!["code", "--wait"]);
        assert_eq!(
            split(r#""C:\Program Files\Notepad++\notepad++.exe" -multiInst"#),
            vec![r"C:\Program Files\Notepad++\notepad++.exe", "-multiInst"]
        );
        assert_eq!(split("  vim  "), vec!["vim"]);
        assert_eq!(split("emacs -nw ''"), vec!["emacs", "-nw", ""]);
        assert!(split("   ").is_empty());
    }

    #[test]
    fn the_editor_comes_from_the_flag_then_the_environment_then_a_default() {
        let sudo = |k: &str| match k {
            "SUDO_EDITOR" => Some("vi".to_string()),
            "VISUAL" => Some("code --wait".to_string()),
            _ => None,
        };
        assert_eq!(choose(None, sudo, |_: &str| true).unwrap(), vec!["vi"]);
        let env = |k: &str| match k {
            "VISUAL" => Some("code --wait".to_string()),
            "EDITOR" => Some("vim".to_string()),
            _ => None,
        };
        let none = |_: &str| None;
        let editor_only = |k: &str| (k == "EDITOR").then(|| "vim".to_string());
        let all = |_: &str| true;
        assert_eq!(choose(Some("nano"), env, all).unwrap(), vec!["nano"]);
        assert_eq!(choose(None, env, all).unwrap(), vec!["code", "--wait"]);
        assert_eq!(choose(None, editor_only, all).unwrap(), vec!["vim"]);
        assert_eq!(
            choose(None, none, |e: &str| e == FALLBACKS[FALLBACKS.len() - 1]).unwrap(),
            vec![FALLBACKS[FALLBACKS.len() - 1]]
        );
        assert!(choose(None, none, |_: &str| false).is_err());
    }
}
