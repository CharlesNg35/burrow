use std::path::{Path, PathBuf};

use crate::config::{Role, load_file};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Service { name: String, running: bool },
    Running { pid: u32 },
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub role: Role,
    pub origin: Origin,
}

pub struct Service {
    pub name: String,
    pub role: Role,
    pub config: PathBuf,
    pub running: bool,
}

pub struct Sources {
    pub services: Vec<Service>,
    pub running: Vec<(u32, PathBuf)>,
    pub folder: Vec<(PathBuf, Role)>,
}

pub fn all() -> Vec<Found> {
    let services = crate::service::installed()
        .into_iter()
        .map(|i| Service {
            role: i.manifest.role,
            name: i.manifest.name,
            config: i.manifest.config,
            running: i.running,
        })
        .collect();
    let running = crate::instance::running()
        .into_iter()
        .map(|r| (r.pid, r.config))
        .collect();
    let folder = crate::config::discover::in_folder(Path::new("."));
    merge(
        Sources {
            services,
            running,
            folder,
        },
        role_of,
    )
}

pub fn role_of(path: &Path) -> Option<Role> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return None;
    }
    let nothing = |_: &str| None;
    load_file(path, None, &nothing).ok()?.role
}

fn canonical(path: &Path) -> PathBuf {
    crate::base::fs::canonical_or_same(path)
}

pub fn merge(sources: Sources, role_of: impl Fn(&Path) -> Option<Role>) -> Vec<Found> {
    let named: Vec<(PathBuf, Role)> = sources
        .folder
        .iter()
        .map(|(p, r)| (canonical(p), *r))
        .collect();
    let mut found: Vec<Found> = Vec::new();
    let mut add = |path: &Path, role: Option<Role>, origin: Origin| {
        let path = canonical(path);
        if found.iter().any(|f| f.path == path) {
            return;
        }
        let by_name = || named.iter().find(|(p, _)| *p == path).map(|(_, r)| *r);
        if let Some(role) = role.or_else(by_name).or_else(|| role_of(&path)) {
            found.push(Found { path, role, origin });
        }
    };
    for s in sources.services {
        let origin = Origin::Service {
            name: s.name,
            running: s.running,
        };
        add(&s.config, Some(s.role), origin);
    }
    for (pid, config) in sources.running {
        add(&config, None, Origin::Running { pid });
    }
    for (path, role) in sources.folder {
        add(&path, Some(role), Origin::Folder);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, "").unwrap();
        canonical(&p)
    }

    #[test]
    fn each_config_shows_once_with_what_uses_it() {
        let dir = tempfile::tempdir().unwrap();
        let installed = file(dir.path(), "installed-client.yaml");
        let adhoc = file(dir.path(), "adhoc.yaml");
        let here = file(dir.path(), "server.yaml");
        let unknown = file(dir.path(), "notes.yaml");
        let found = merge(
            Sources {
                services: vec![Service {
                    name: "burrow-home".into(),
                    role: Role::Client,
                    config: installed.clone(),
                    running: true,
                }],
                running: vec![
                    (41, installed.clone()),
                    (42, adhoc.clone()),
                    (43, here.clone()),
                    (44, unknown),
                ],
                folder: vec![(here.clone(), Role::Server)],
            },
            |p| p.ends_with("adhoc.yaml").then_some(Role::Visitor),
        );
        assert_eq!(
            found,
            vec![
                Found {
                    path: installed,
                    role: Role::Client,
                    origin: Origin::Service {
                        name: "burrow-home".into(),
                        running: true
                    },
                },
                Found {
                    path: adhoc,
                    role: Role::Visitor,
                    origin: Origin::Running { pid: 42 },
                },
                Found {
                    path: here,
                    role: Role::Server,
                    origin: Origin::Running { pid: 43 },
                },
            ]
        );
    }
}
