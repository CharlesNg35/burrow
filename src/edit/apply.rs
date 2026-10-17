use std::io::Write;

use super::found::{Found, Origin};
use crate::config::Role;
use crate::failure::{Failure, admin};

#[derive(Debug, PartialEq, Eq)]
pub enum After {
    Reloads { role: Role, service: Option<String> },
    NextStart(String),
    Nothing,
}

pub fn after(f: &Found) -> After {
    match &f.origin {
        Origin::Service {
            name,
            running: true,
        } => After::Reloads {
            role: f.role,
            service: Some(name.clone()),
        },
        Origin::Running { .. } => After::Reloads {
            role: f.role,
            service: None,
        },
        Origin::Service {
            name,
            running: false,
        } => After::NextStart(name.clone()),
        Origin::Folder => After::Nothing,
    }
}

fn say(out: &mut dyn Write, line: impl AsRef<str>) -> Result<(), Failure> {
    writeln!(out, "{}", line.as_ref()).map_err(|e| Failure::new(format!("cannot write: {e}")))
}

pub fn apply(after: After, out: &mut dyn Write) -> Result<(), Failure> {
    match after {
        After::Reloads { role, service } => {
            say(
                out,
                format!(
                    "the {} applies it by itself and keeps its current config if it finds a problem; open connections keep running",
                    role.as_str()
                ),
            )?;
            if let Some(name) = service {
                say(
                    out,
                    format!("  see it in: {}", admin(&format!("service logs {name}"))),
                )?;
            }
            Ok(())
        }
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
    fn whatever_runs_the_config_applies_it_by_itself() {
        for role in [Role::Server, Role::Client, Role::Visitor] {
            assert_eq!(
                after(&found(role, service(true))),
                After::Reloads {
                    role,
                    service: Some("burrow-home".into())
                }
            );
            assert_eq!(
                after(&found(role, Origin::Running { pid: 7 })),
                After::Reloads {
                    role,
                    service: None
                }
            );
            assert_eq!(
                after(&found(role, service(false))),
                After::NextStart("burrow-home".into())
            );
            assert_eq!(after(&found(role, Origin::Folder)), After::Nothing);
        }
    }

    #[test]
    fn the_message_names_the_role_and_where_to_look() {
        let mut out = Vec::new();
        apply(
            After::Reloads {
                role: Role::Client,
                service: Some("burrow-home".into()),
            },
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("the client applies it by itself"),
            "{text}"
        );
        assert!(text.contains("service logs burrow-home"), "{text}");
    }
}
