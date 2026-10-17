use std::path::{Path, PathBuf};

use super::Role;
use crate::failure::Failure;

const EXTENSIONS: [&str; 3] = ["yaml", "yml", "json"];

fn candidates(dir: &Path, role: Role) -> Vec<PathBuf> {
    EXTENSIONS
        .iter()
        .map(|ext| {
            let name = format!("{}.{ext}", role.as_str());
            if dir == Path::new(".") {
                PathBuf::from(name)
            } else {
                dir.join(name)
            }
        })
        .filter(|p| p.is_file())
        .collect()
}

pub fn in_folder(dir: &Path) -> Vec<(PathBuf, Role)> {
    [Role::Server, Role::Client, Role::Visitor]
        .into_iter()
        .flat_map(|role| candidates(dir, role).into_iter().map(move |p| (p, role)))
        .collect()
}

pub fn discover(dir: &Path, role: Option<Role>) -> Result<PathBuf, Failure> {
    let roles = match role {
        Some(r) => vec![r],
        None => vec![Role::Server, Role::Client, Role::Visitor],
    };
    let found: Vec<PathBuf> = roles.iter().flat_map(|r| candidates(dir, *r)).collect();
    let shown = |p: &PathBuf| {
        p.file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    };
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => {
            let wanted = match role {
                Some(r) => format!("{}.yaml", r.as_str()),
                None => "server.yaml, client.yaml or visitor.yaml".into(),
            };
            let init = role.map_or("server", Role::as_str);
            let other = role.and_then(|r| {
                [Role::Server, Role::Client, Role::Visitor]
                    .into_iter()
                    .filter(|o| *o != r)
                    .find_map(|o| candidates(dir, o).first().map(|p| (o, shown(p))))
            });
            let hint = match other {
                Some((o, file)) => format!(
                    "{file} here is a {} config, run it with: {cmd} {}; otherwise pass -c FILE, or create one with: {cmd} init {init}",
                    o.as_str(),
                    o.command(),
                    cmd = crate::failure::command()
                ),
                None => format!(
                    "pass -c FILE, or create one with: {} init {init}",
                    crate::failure::command()
                ),
            };
            Err(Failure::usage(format!("no {wanted} in {}", dir.display())).hint(hint))
        }
        many => Err(Failure::usage(format!(
            "found {} in {}; not sure which one to use",
            many.iter().map(shown).collect::<Vec<_>>().join(" and "),
            dir.display()
        ))
        .hint(format!(
            "pass the one you mean, for example: -c {}",
            shown(&many[0])
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_only_candidate() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.yaml"), "").unwrap();
        assert_eq!(
            discover(dir.path(), None).unwrap(),
            dir.path().join("server.yaml")
        );
        assert_eq!(
            discover(dir.path(), Some(Role::Server)).unwrap(),
            dir.path().join("server.yaml")
        );
        let err = discover(dir.path(), Some(Role::Client)).unwrap_err();
        assert!(
            err.message.starts_with("no client.yaml in"),
            "{}",
            err.message
        );
        let hint = err.hint.unwrap();
        assert!(hint.contains("burrow init client"), "{hint}");
        assert!(
            hint.contains("server.yaml here is a server config, run it with: burrow server"),
            "{hint}"
        );
    }

    #[test]
    fn a_visitor_config_points_to_visit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("visitor.yaml"), "").unwrap();
        let hint = discover(dir.path(), Some(Role::Client))
            .unwrap_err()
            .hint
            .unwrap();
        assert!(
            hint.starts_with("visitor.yaml here is a visitor config, run it with: burrow visit;"),
            "{hint}"
        );
    }

    #[test]
    fn refuses_to_guess_between_two() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.yaml"), "").unwrap();
        std::fs::write(dir.path().join("client.json"), "").unwrap();
        let err = discover(dir.path(), None).unwrap_err();
        assert!(
            err.message.contains("server.yaml and client.json"),
            "{}",
            err.message
        );
        assert_eq!(err.exit, 2);
        assert_eq!(
            discover(dir.path(), Some(Role::Client)).unwrap(),
            dir.path().join("client.json")
        );
    }

    #[test]
    fn empty_folder_points_to_init() {
        let dir = tempfile::tempdir().unwrap();
        let err = discover(dir.path(), None).unwrap_err();
        assert!(
            err.message
                .contains("server.yaml, client.yaml or visitor.yaml")
        );
        let err = discover(dir.path(), Some(Role::Server)).unwrap_err();
        assert_eq!(
            err.hint.unwrap(),
            "pass -c FILE, or create one with: burrow init server"
        );
    }
}
