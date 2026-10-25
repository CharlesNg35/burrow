use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::base::failure::Failure;
use crate::config::Role;
use crate::config::env::Env;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub config: PathBuf,
    pub env_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    File(PathBuf),
    Service(Installed),
}

fn same(name: &str, wanted: &str) -> bool {
    name == wanted || crate::install::short_name(name) == wanted
}

pub fn choose(
    given: Option<PathBuf>,
    here: Option<PathBuf>,
    services: &[Installed],
    pick: Option<&str>,
) -> Result<Found, Failure> {
    if let Some(p) = given {
        return Ok(Found::File(p));
    }
    if let Some(wanted) = pick {
        return services
            .iter()
            .find(|s| same(&s.name, wanted))
            .cloned()
            .map(Found::Service)
            .ok_or_else(|| {
                let names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
                Failure::usage(format!("no installed visitor service named {wanted}")).hint(
                    if names.is_empty() {
                        "no visitor service is installed; pass -c FILE".to_string()
                    } else {
                        format!("installed visitor services: {}", names.join(", "))
                    },
                )
            });
    }
    if let Some(p) = here {
        return Ok(Found::File(p));
    }
    match services {
        [one] => Ok(Found::Service(one.clone())),
        [] => Err(Failure::usage("no visitor config found").hint(format!(
            "pass -c FILE, create one with: {}, or install a visitor service",
            crate::base::failure::cmd("init visitor")
        ))),
        many => Err(Failure::usage(format!(
            "several visitor services are installed: {}",
            many.iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .hint("pick one with --service NAME, or pass -c FILE")),
    }
}

pub fn unreadable(what: &std::path::Path, e: &std::io::Error, command: &str) -> Failure {
    let hint = if cfg!(windows) {
        format!("run burrow {command} from an administrator terminal, or pass -c FILE")
    } else {
        format!(
            "run with sudo ({}), or pass -c FILE",
            crate::base::failure::admin(command)
        )
    };
    Failure::new(format!(
        "cannot read {}: {}",
        what.display(),
        crate::base::failure::file_reason(e)
    ))
    .hint(hint)
}

pub struct Chosen {
    pub path: PathBuf,
    pub env: Env,
    pub note: Option<String>,
}

fn readable(p: &Path, command: &str) -> Result<(), Failure> {
    std::fs::File::open(p)
        .map(drop)
        .map_err(|e| unreadable(p, &e, command))
}

pub fn visitor(
    config: Option<PathBuf>,
    service: Option<&str>,
    command: &str,
    saved_env: fn(&str) -> BTreeMap<String, String>,
) -> Result<Chosen, Failure> {
    let here = crate::config::discover::in_folder(Path::new("."))
        .into_iter()
        .filter(|(_, r)| *r == Role::Visitor)
        .map(|(p, _)| p)
        .next();
    let services: Vec<Installed> = if config.is_none() && (here.is_none() || service.is_some()) {
        crate::install::all_manifests()
            .into_iter()
            .filter(|m| m.role == "visitor")
            .map(|m| Installed {
                name: m.name,
                config: m.config,
                env_file: m.env_file,
            })
            .collect()
    } else {
        Vec::new()
    };
    match choose(config, here, &services, service)? {
        Found::File(path) => Ok(Chosen {
            path,
            env: Env::default(),
            note: None,
        }),
        Found::Service(s) => {
            readable(&s.config, command)?;
            let env = match &s.env_file {
                Some(f) => {
                    readable(f, command)?;
                    Env::with(saved_env(&s.name))
                }
                None => Env::default(),
            };
            Ok(Chosen {
                note: Some(format!(
                    "using the installed visitor service {} ({})",
                    s.name,
                    s.config.display()
                )),
                path: s.config,
                env,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(name: &str) -> Installed {
        Installed {
            name: name.into(),
            config: format!("/etc/burrow/{name}.yaml").into(),
            env_file: None,
        }
    }

    #[test]
    fn the_lookup_order_is_flag_then_folder_then_one_service() {
        let flag = Some(PathBuf::from("mine.yaml"));
        let here = Some(PathBuf::from("./visitor.yaml"));
        assert_eq!(
            choose(flag.clone(), here.clone(), &[svc("burrow-a")], None).unwrap(),
            Found::File("mine.yaml".into())
        );
        assert_eq!(
            choose(None, here.clone(), &[svc("burrow-a")], None).unwrap(),
            Found::File("./visitor.yaml".into()),
            "a config in the current folder wins over an installed service"
        );
        assert_eq!(
            choose(None, None, &[svc("burrow-a")], None).unwrap(),
            Found::Service(svc("burrow-a"))
        );
        let several = choose(None, None, &[svc("burrow-a"), svc("burrow-b")], None).unwrap_err();
        assert!(several.message.contains("burrow-a, burrow-b"));
        assert_eq!(
            several.hint.as_deref(),
            Some("pick one with --service NAME, or pass -c FILE")
        );
        assert_eq!(
            choose(None, None, &[svc("burrow-a"), svc("burrow-b")], Some("b")).unwrap(),
            Found::Service(svc("burrow-b"))
        );
        assert_eq!(
            choose(
                None,
                here,
                &[svc("burrow-a"), svc("burrow-b")],
                Some("burrow-a")
            )
            .unwrap(),
            Found::Service(svc("burrow-a")),
            "--service wins over the folder"
        );
        let missing = choose(None, None, &[svc("burrow-a")], Some("c")).unwrap_err();
        assert!(missing.hint.unwrap().contains("burrow-a"));
        let nothing = choose(None, None, &[], None).unwrap_err();
        assert!(
            nothing
                .hint
                .unwrap()
                .contains("or install a visitor service")
        );
        let e = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let u = unreadable(std::path::Path::new("/etc/burrow/a.env"), &e, "mcp");
        let hint = u.hint.unwrap();
        assert!(hint.contains("mcp"), "{hint}");
        assert!(hint.contains(if cfg!(windows) {
            "administrator"
        } else {
            "sudo"
        }));
    }
}
