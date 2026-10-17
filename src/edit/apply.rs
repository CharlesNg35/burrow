use std::io::Write;

use super::found::{Found, Origin};
use crate::config::Role;
use crate::failure::{Failure, admin};
use crate::prompt::Prompter;

#[derive(Debug, PartialEq, Eq)]
pub enum After {
    Reloads(Option<String>),
    Restart(String),
    RestartYourself(u32),
    NextStart(String),
    Nothing,
}

pub fn after(f: &Found) -> After {
    match (&f.origin, f.role) {
        (
            Origin::Service {
                name,
                running: true,
            },
            Role::Server,
        ) => After::Reloads(Some(name.clone())),
        (Origin::Running { .. }, Role::Server) => After::Reloads(None),
        (
            Origin::Service {
                name,
                running: true,
            },
            _,
        ) => After::Restart(name.clone()),
        (Origin::Running { pid }, _) => After::RestartYourself(*pid),
        (
            Origin::Service {
                name,
                running: false,
            },
            _,
        ) => After::NextStart(name.clone()),
        (Origin::Folder, _) => After::Nothing,
    }
}

fn say(out: &mut dyn Write, line: impl AsRef<str>) -> Result<(), Failure> {
    writeln!(out, "{}", line.as_ref()).map_err(|e| Failure::new(format!("cannot write: {e}")))
}

pub fn apply(after: After, prompt: &mut dyn Prompter, out: &mut dyn Write) -> Result<(), Failure> {
    match after {
        After::Reloads(service) => {
            say(
                out,
                "the server reloads it by itself, and keeps its current config if it finds a problem",
            )?;
            if let Some(name) = service {
                say(
                    out,
                    format!("  see it in: {}", admin(&format!("service logs {name}"))),
                )?;
            }
            Ok(())
        }
        After::Restart(name) => {
            let restart = admin(&format!("service restart {name}"));
            if !prompt.interactive() {
                return say(out, format!("{name} uses it after a restart: {restart}"));
            }
            if prompt.confirm(&format!("restart {name} now to use it?"), true) {
                crate::service::control("restart", Some(&name), out)
            } else {
                say(out, format!("{name} keeps its old config until: {restart}"))
            }
        }
        After::RestartYourself(pid) => say(
            out,
            format!("the instance running as pid {pid} uses it after you restart it"),
        ),
        After::NextStart(name) => say(
            out,
            format!(
                "{name} is stopped; it uses it when it starts: {}",
                admin(&format!("service start {name}"))
            ),
        ),
        After::Nothing => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn found(role: Role, origin: Origin) -> Found {
        Found {
            path: PathBuf::from("/etc/burrow/x.yaml"),
            role,
            origin,
        }
    }

    fn service(running: bool) -> Origin {
        Origin::Service {
            name: "burrow-home".into(),
            running,
        }
    }

    #[test]
    fn servers_reload_by_themselves_and_the_others_need_a_restart() {
        assert_eq!(
            after(&found(Role::Server, service(true))),
            After::Reloads(Some("burrow-home".into()))
        );
        assert_eq!(
            after(&found(Role::Server, Origin::Running { pid: 7 })),
            After::Reloads(None)
        );
        assert_eq!(
            after(&found(Role::Client, service(true))),
            After::Restart("burrow-home".into())
        );
        assert_eq!(
            after(&found(Role::Visitor, Origin::Running { pid: 7 })),
            After::RestartYourself(7)
        );
        assert_eq!(
            after(&found(Role::Client, service(false))),
            After::NextStart("burrow-home".into())
        );
        assert_eq!(
            after(&found(Role::Server, service(false))),
            After::NextStart("burrow-home".into())
        );
        assert_eq!(after(&found(Role::Client, Origin::Folder)), After::Nothing);
    }

    #[test]
    fn without_a_terminal_a_restart_is_only_suggested() {
        let mut out = Vec::new();
        apply(
            After::Restart("burrow-home".into()),
            &mut crate::prompt::NoPrompt,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("service restart burrow-home"), "{text}");
    }
}
