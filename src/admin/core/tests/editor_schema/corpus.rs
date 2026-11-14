#![cfg(test)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::emit::render;
use crate::config::{Format, load::detect_role, tree};

const SECRET: &str = "yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=";

fn pages(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let path = e.path();
        if path.is_dir() {
            pages(&path, out);
        } else if path.extension().is_some_and(|x| x == "md") {
            out.push(path);
        }
    }
}

fn blocks(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for line in md.lines() {
        let trimmed = line.trim_start();
        match open.as_mut() {
            None if trimmed == "```yaml" => {
                open = Some((line.len() - trimmed.len(), String::new()))
            }
            None => {}
            Some(_) if trimmed == "```" => out.extend(open.take().map(|(_, t)| t)),
            Some((indent, text)) => {
                text.push_str(line.get(*indent..).unwrap_or(""));
                text.push('\n');
            }
        }
    }
    out
}

fn server_config(text: &str) -> bool {
    let first = text.lines().next().unwrap_or("");
    if first.starts_with("# client.yaml") || first.starts_with("# visitor.yaml") {
        return false;
    }
    if first.starts_with("# server.yaml") || first.starts_with("# server.d/") {
        return true;
    }
    match tree::parse(text) {
        Ok(t) if matches!(t.value, tree::Value::Map(_)) => {
            detect_role(&t) == Some(crate::config::Role::Server)
        }
        _ => false,
    }
}

pub(super) fn configs() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("README.md")];
    pages(&root.join("docs"), &mut files);
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let md = std::fs::read_to_string(&f).unwrap();
        let name = f.strip_prefix(root).unwrap().display().to_string();
        for (i, b) in blocks(&md).into_iter().enumerate() {
            if server_config(&b) {
                out.push((format!("{name} block {i}"), b));
            }
        }
    }
    let dev = std::fs::read_to_string(root.join("dev/server.yaml")).unwrap();
    out.push(("dev/server.yaml".into(), dev));
    out.push((
        "burrow init".into(),
        render(&crate::init::server_value("home", SECRET), Format::Yaml),
    ));
    out.push(("the world of the admin tests".into(), super::world().text));
    out
}

pub(super) fn env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for (k, v) in [
        ("HOME_SECRET", SECRET),
        (
            "OFFICE_SECRET",
            "0uAEjsD0O5lWx7ngXPdRMpdMvjrVqA5IFw1MQf2EYxA=",
        ),
        (
            "LAPTOP_SECRET",
            "YoO7pXZ3rpxdzO0+O+KzyqPc+EzxviqjbPn7J/7S0eU=",
        ),
        ("CF_TOKEN", "cf-token"),
        ("CF_API_TOKEN", "cf-token"),
    ] {
        env.insert(k.to_string(), v.to_string());
    }
    env
}
