use std::fmt;

use crate::diagnostics::Style;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
    pub hint: Option<String>,
    pub exit: u8,
}

pub const EXIT_FAILURE: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_CONFIG: u8 = 3;

impl Failure {
    pub fn new(message: impl Into<String>) -> Self {
        Failure {
            message: message.into(),
            hint: None,
            exit: EXIT_FAILURE,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Failure {
            exit: EXIT_USAGE,
            ..Failure::new(message)
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn render(&self, style: Style) -> String {
        let (head, hint) = if style.color {
            ("\x1b[1;31merror\x1b[0m\x1b[1m:", "\x1b[1;34mhint:\x1b[0m")
        } else {
            ("error:", "hint:")
        };
        let mut s = format!("{head} {}", self.message);
        if style.color {
            s.push_str("\x1b[0m");
        }
        if let Some(h) = &self.hint {
            s.push_str(&format!("\n  {hint} {h}"));
        }
        s
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(Style { color: false }))
    }
}

impl std::error::Error for Failure {}

pub fn io_reason(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    let s = match e.kind() {
        K::ConnectionRefused => "connection refused",
        K::ConnectionReset => "connection reset",
        K::ConnectionAborted => "connection aborted",
        K::TimedOut => "timed out",
        K::NotFound => "host not found",
        K::AddrInUse => "address already in use",
        K::AddrNotAvailable => "address not available",
        K::PermissionDenied => "permission denied",
        K::UnexpectedEof => "connection closed unexpectedly",
        K::BrokenPipe => "connection closed",
        K::HostUnreachable => "host unreachable",
        K::NetworkUnreachable => "network unreachable",
        _ => {
            let text = e.to_string();
            return text
                .split(" (os error")
                .next()
                .unwrap_or(&text)
                .to_lowercase();
        }
    };
    s.to_string()
}

pub fn file_reason(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => "file not found".into(),
        K::PermissionDenied => "permission denied".into(),
        K::IsADirectory => "is a directory, not a file".into(),
        _ => io_reason(e),
    }
}

fn on_path(exe: &std::path::Path) -> bool {
    let name = if cfg!(windows) {
        "burrow.exe"
    } else {
        "burrow"
    };
    let Ok(me) = std::fs::canonicalize(exe) else {
        return false;
    };
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .any(|dir| std::fs::canonicalize(dir.join(name)).is_ok_and(|p| p == me))
    })
}

fn command_for(arg0: Option<&std::ffi::OsStr>) -> String {
    let Some(path) = arg0.map(std::path::Path::new) else {
        return "burrow".into();
    };
    let typed_path = path.components().count() > 1;
    let is_burrow = path.file_stem().is_some_and(|s| s == "burrow");
    if !typed_path || !is_burrow || on_path(path) {
        return "burrow".into();
    }
    let shown = path.display().to_string();
    if shown.contains(' ') {
        if cfg!(windows) {
            format!("& \"{shown}\"")
        } else {
            format!("'{shown}'")
        }
    } else {
        shown
    }
}

pub fn command() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| command_for(std::env::args_os().next().as_deref()))
}

pub fn cmd(rest: &str) -> String {
    format!("{} {rest}", command())
}

pub fn admin(rest: &str) -> String {
    if cfg!(windows) {
        format!("{} {rest} (as administrator)", command())
    } else {
        format!("sudo {} {rest}", command())
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn hints_use_the_command_the_user_typed() {
        assert_eq!(command_for(None), "burrow");
        assert_eq!(command_for(Some(OsStr::new("burrow"))), "burrow");
        assert_eq!(command_for(Some(OsStr::new("./burrow"))), "./burrow");
        assert_eq!(
            command_for(Some(OsStr::new("/opt/burrow 1/burrow"))),
            if cfg!(windows) {
                "& \"/opt/burrow 1/burrow\""
            } else {
                "'/opt/burrow 1/burrow'"
            }
        );
        assert_eq!(
            command_for(Some(OsStr::new("target/debug/deps/integration-1234"))),
            "burrow"
        );
    }
}
