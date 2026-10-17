use std::io::IsTerminal;
use std::path::Path;

use super::found::{Found, Origin, role_of};
use crate::config::Role;
use crate::failure::Failure;
use crate::prompt::Prompter;

pub fn place(f: &Found) -> String {
    match &f.origin {
        Origin::Service {
            name,
            running: true,
        } => format!("service {name}, running"),
        Origin::Service {
            name,
            running: false,
        } => format!("service {name}, stopped"),
        Origin::Running { pid } => format!("running, pid {pid}"),
        Origin::Folder => "this folder".into(),
    }
}

pub fn table(found: &[Found]) -> String {
    let rows: Vec<[String; 4]> = found
        .iter()
        .enumerate()
        .map(|(i, f)| {
            [
                (i + 1).to_string(),
                f.role.as_str().to_string(),
                place(f),
                f.path.display().to_string(),
            ]
        })
        .collect();
    let head = ["#", "ROLE", "USED BY", "CONFIG"].map(String::from);
    let widths: Vec<usize> = (0..4)
        .map(|c| {
            rows.iter()
                .chain(std::iter::once(&head))
                .map(|r| r[c].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    std::iter::once(&head)
        .chain(rows.iter())
        .map(|r| {
            let line: Vec<String> = r
                .iter()
                .zip(&widths)
                .map(|(cell, w)| format!("{cell:<w$}"))
                .collect();
            format!("  {}\n", line.join("  ").trim_end())
        })
        .collect()
}

fn listed(found: &[Found]) -> String {
    if found.is_empty() {
        return "no burrow config was found here, in the installed services or in running instances".into();
    }
    format!("burrow edit can open:\n{}", table(found))
}

pub fn by_target(found: &[Found], target: &str) -> Result<Found, Failure> {
    let path = Path::new(target);
    if path.is_file() {
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(f) = found.iter().find(|f| f.path == real) {
            return Ok(f.clone());
        }
        let role = role_of(&real).ok_or_else(|| {
            Failure::usage(format!(
                "{target} is not a burrow server, client or visitor config"
            ))
            .hint(format!(
                "check it with: {}",
                crate::failure::cmd(&format!("validate -c {target}"))
            ))
        })?;
        return Ok(Found {
            path: real,
            role,
            origin: Origin::Folder,
        });
    }
    let full = format!("burrow-{target}");
    let service = found.iter().find(
        |f| matches!(&f.origin, Origin::Service { name, .. } if *name == target || *name == full),
    );
    if let Some(f) = service {
        return Ok(f.clone());
    }
    if let Some(role) = Role::parse(target) {
        let same: Vec<Found> = found.iter().filter(|f| f.role == role).cloned().collect();
        return match same.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(Failure::usage(format!("no {target} config was found")).hint(listed(found))),
            _ => Err(Failure::usage(format!(
                "found {} {target} configs; not sure which one to open",
                same.len()
            ))
            .hint(format!("pass a service name or a path\n{}", table(&same)))),
        };
    }
    Err(Failure::usage(format!("no config, service or file called {target}")).hint(listed(found)))
}

pub fn ask(found: &[Found], prompt: &mut dyn Prompter) -> Result<Found, Failure> {
    let none = || {
        Failure::usage("found several burrow configs; not sure which one to open").hint(format!(
            "pass one, for example: {}\n{}",
            crate::failure::cmd("edit client"),
            table(found)
        ))
    };
    if !prompt.interactive() {
        return Err(none());
    }
    let text = table(found);
    let style = crate::diagnostics::Style::detect(std::io::stderr().is_terminal());
    let (head, rows) = text.split_once('\n').unwrap_or((&text, ""));
    eprint!("{}\n{rows}", style.paint("1", head));
    for _ in 0..3 {
        let answer = prompt.ask(&format!("open which one? [1-{}]", found.len()));
        let Some(answer) = answer else {
            return Err(none());
        };
        match answer.parse::<usize>() {
            Ok(n) if (1..=found.len()).contains(&n) => return Ok(found[n - 1].clone()),
            _ => eprintln!("type a number from 1 to {}", found.len()),
        }
    }
    Err(none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample() -> Vec<Found> {
        vec![
            Found {
                path: PathBuf::from("/etc/burrow/server.yaml"),
                role: Role::Server,
                origin: Origin::Service {
                    name: "burrow-server".into(),
                    running: true,
                },
            },
            Found {
                path: PathBuf::from("/etc/burrow/client-home.yaml"),
                role: Role::Client,
                origin: Origin::Service {
                    name: "burrow-home".into(),
                    running: false,
                },
            },
            Found {
                path: PathBuf::from("/tmp/lab/client.yaml"),
                role: Role::Client,
                origin: Origin::Running { pid: 4123 },
            },
        ]
    }

    struct Answers(Vec<&'static str>);

    impl Prompter for Answers {
        fn interactive(&self) -> bool {
            true
        }
        fn ask(&mut self, _: &str) -> Option<String> {
            (!self.0.is_empty()).then(|| self.0.remove(0).to_string())
        }
    }

    #[test]
    fn a_config_is_picked_by_service_name_or_role() {
        let found = sample();
        assert_eq!(by_target(&found, "home").unwrap(), found[1]);
        assert_eq!(by_target(&found, "burrow-home").unwrap(), found[1]);
        assert_eq!(by_target(&found, "server").unwrap(), found[0]);
        let several = by_target(&found, "client").unwrap_err();
        assert!(
            several.message.contains("2 client configs"),
            "{}",
            several.message
        );
        let none = by_target(&found, "office").unwrap_err();
        assert!(none.hint.unwrap().contains("burrow-home"));
        assert!(by_target(&found, "visitor").is_err());
    }

    #[test]
    fn the_list_asks_until_it_gets_a_number_in_range() {
        let found = sample();
        let mut answers = Answers(vec!["x", "9", "3"]);
        assert_eq!(ask(&found, &mut answers).unwrap(), found[2]);
        assert!(ask(&found, &mut Answers(vec![])).is_err());
        assert!(ask(&found, &mut crate::prompt::NoPrompt).is_err());
    }

    #[test]
    fn the_table_says_what_uses_each_config() {
        let text = table(&sample());
        assert!(text.contains("service burrow-server, running"));
        assert!(text.contains("service burrow-home, stopped"));
        assert!(text.contains("running, pid 4123"));
        assert!(text.lines().all(|l| l.starts_with("  ")));
    }
}
