use std::path::Path;
use std::process::Command;

use crate::shape::lex;

pub const THRESHOLD: &str = "too-many-lines-threshold = 150";
pub const SILENCED: &[&str] = &["too_many_lines", "pedantic", "all"];
pub const FLAG_OVERRIDES: &[&str] = &[
    "-A clippy",
    "-Aclippy",
    "--allow clippy",
    "--allow=clippy",
    "-W clippy::too_many_lines",
    "-Wclippy::too_many_lines",
    "--warn clippy::too_many_lines",
    "--warn=clippy::too_many_lines",
];

#[derive(Debug, Clone, PartialEq)]
struct Tok {
    text: String,
    line: usize,
}

fn tokens(code: &str) -> Vec<Tok> {
    let chars: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1);
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok {
                text: chars[start..i].iter().collect(),
                line,
            });
        } else if c == ':' && chars.get(i + 1) == Some(&':') {
            out.push(Tok {
                text: "::".into(),
                line,
            });
            i += 2;
        } else {
            out.push(Tok {
                text: c.to_string(),
                line,
            });
            i += 1;
        }
    }
    out
}

fn closing(toks: &[Tok], open: usize) -> usize {
    let (o, c) = match toks[open].text.as_str() {
        "(" => ("(", ")"),
        "[" => ("[", "]"),
        _ => ("{", "}"),
    };
    let mut depth = 0;
    for (k, t) in toks.iter().enumerate().skip(open) {
        if t.text == o {
            depth += 1;
        } else if t.text == c {
            depth -= 1;
            if depth == 0 {
                return k;
            }
        }
    }
    toks.len()
}

fn texts(toks: &[Tok]) -> Vec<&str> {
    toks.iter().map(|t| t.text.as_str()).collect()
}

pub fn silenced_problems(path: &str, src: &str) -> Vec<String> {
    let toks = tokens(&lex(src).code);
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let at = if toks[i].text == "#" && toks.get(i + 1).is_some_and(|t| t.text == "!") {
            i + 2
        } else {
            i + 1
        };
        if toks[i].text != "#" || toks.get(at).is_none_or(|t| t.text != "[") {
            i += 1;
            continue;
        }
        let end = closing(&toks, at);
        let attr = &toks[at..end.min(toks.len())];
        for (k, t) in attr.iter().enumerate() {
            let lowers = ["allow", "expect", "warn"].contains(&t.text.as_str());
            if !lowers || attr.get(k + 1).is_none_or(|n| n.text != "(") {
                continue;
            }
            let inner = &attr[k + 1..closing(attr, k + 1).min(attr.len())];
            let words = texts(inner);
            let named = words
                .windows(3)
                .any(|w| w[0] == "clippy" && w[1] == "::" && SILENCED.contains(&w[2]));
            let all = t.text != "warn"
                && words
                    .iter()
                    .enumerate()
                    .any(|(n, w)| *w == "warnings" && (n == 0 || words[n - 1] != "::"));
            if named || all {
                out.push(format!(
                    "{path}:{}: {}({}) would silence clippy::too_many_lines; split the function into named steps",
                    t.line,
                    t.text,
                    words[1..].concat()
                ));
            }
        }
        i = end;
    }
    out
}

pub fn alias_problems(path: &str, src: &str) -> Vec<String> {
    let toks = tokens(&lex(src).code);
    let words = texts(&toks);
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let next = |n: usize| words.get(i + n).copied().unwrap_or_default();
        let found = match *w {
            "extern" if next(1) == "crate" => Some("extern crate"),
            "use" if next(1) == "crate" && next(2) == "as" => Some("use crate as"),
            "crate" if next(1) == "::" && next(2) == "{" => {
                let end = closing(&toks, i + 2);
                words[i + 2..end.min(words.len())]
                    .windows(2)
                    .any(|p| p == ["self", "as"])
                    .then_some("crate::{self as ..}")
            }
            _ => None,
        };
        if let Some(what) = found {
            out.push(format!(
                "{path}:{}: {what} gives the crate another name; use crate:: paths",
                toks[i].line
            ));
        }
    }
    out
}

fn section<'a>(toml: &'a str, header: &str) -> Option<Vec<&'a str>> {
    let mut lines = toml.lines().map(str::trim);
    lines.find(|l| *l == header)?;
    Some(
        lines
            .take_while(|l| !l.starts_with('['))
            .filter(|l| !l.is_empty())
            .collect(),
    )
}

pub fn members(root_toml: &str) -> Vec<String> {
    let Some(start) = root_toml.find("members = [") else {
        return Vec::new();
    };
    let rest = &root_toml[start + "members = [".len()..];
    rest[..rest.find(']').unwrap_or(rest.len())]
        .split(',')
        .map(|m| m.trim().trim_matches('"').to_string())
        .filter(|m| !m.is_empty())
        .collect()
}

pub fn member_lint_problems(path: &str, toml: &str) -> Vec<String> {
    let headers: Vec<&str> = toml
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("[lints"))
        .collect();
    let plain =
        headers == ["[lints]"] && section(toml, "[lints]") == Some(vec!["workspace = true"]);
    if plain {
        Vec::new()
    } else {
        vec![format!(
            "{path}: the [lints] table must be exactly `workspace = true`, so the workspace lints apply"
        )]
    }
}

pub fn root_problems(cargo: &str, clippy: &str) -> Vec<String> {
    let mut out = Vec::new();
    if !clippy.lines().any(|l| l.trim() == THRESHOLD) {
        out.push(format!("clippy.toml must say `{THRESHOLD}`"));
    }
    let deny = section(cargo, "[workspace.lints.clippy]")
        .is_some_and(|s| s.contains(&"too_many_lines = \"deny\""));
    if !deny {
        out.push(
            "Cargo.toml must set too_many_lines = \"deny\" in [workspace.lints.clippy]".into(),
        );
    }
    out.extend(workspace_lint_problems(cargo));
    out
}

fn level_of(value: &str) -> String {
    let value = value.trim();
    let level = match value.strip_prefix('{') {
        Some(table) => table
            .split(',')
            .find_map(|kv| kv.trim().strip_prefix("level").map(str::to_string))
            .unwrap_or_default(),
        None => value.to_string(),
    };
    level
        .trim()
        .trim_start_matches('=')
        .trim()
        .trim_matches(|c| c == '"' || c == '}' || c == ' ')
        .to_string()
}

pub fn lint_levels(cargo: &str, tool: &str) -> Vec<(String, String)> {
    let table = format!("[workspace.lints.{tool}]");
    let sub = format!("[workspace.lints.{tool}.");
    let dotted = format!("{tool}.");
    let mut out = Vec::new();
    let mut current = String::new();
    for line in cargo.lines().map(str::trim) {
        if line.starts_with('[') {
            current = line.to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"');
        if current == table {
            out.push((key.to_string(), level_of(value)));
        } else if current == "[workspace.lints]"
            && let Some(name) = key.strip_prefix(&dotted)
        {
            out.push((name.trim_matches('"').to_string(), level_of(value)));
        } else if let Some(name) = current.strip_prefix(&sub).and_then(|n| n.strip_suffix(']'))
            && key == "level"
        {
            out.push((name.trim_matches('"').to_string(), level_of(value)));
        }
    }
    out
}

pub fn workspace_lint_problems(cargo: &str) -> Vec<String> {
    let lowered = |level: &str| matches!(level, "allow" | "warn" | "expect");
    let clippy = lint_levels(cargo, "clippy")
        .into_iter()
        .filter(|(k, l)| SILENCED.contains(&k.as_str()) && lowered(l))
        .map(|(k, l)| {
            format!(
                "Cargo.toml: workspace lint clippy::{k} = {l} would silence clippy::too_many_lines"
            )
        });
    let rust = lint_levels(cargo, "rust")
        .into_iter()
        .filter(|(k, l)| k == "warnings" && matches!(l.as_str(), "allow" | "expect"))
        .map(|(k, l)| format!("Cargo.toml: workspace lint {k} = {l} would silence every lint"));
    clippy.chain(rust).collect()
}

pub fn flag_problems(path: &str, text: &str) -> Vec<String> {
    let plain: String = text
        .chars()
        .filter(|c| !matches!(c, '"' | '\'' | ','))
        .collect();
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    FLAG_OVERRIDES
        .iter()
        .filter(|f| plain.contains(*f))
        .map(|f| format!("{path}: `{f}` would loosen the clippy rules"))
        .collect()
}

fn listed(root: &Path) -> Vec<String> {
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let mut files: Vec<String> = run(&["ls-files"])
        .lines()
        .chain(run(&["ls-files", "--others", "--exclude-standard"]).lines())
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    files
}

pub fn problems(root: &Path) -> Result<Vec<String>, String> {
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_default();
    let mut out = Vec::new();
    for path in crate::shape::tracked(root)? {
        let src = read(&path);
        out.extend(silenced_problems(&path, &src));
        if path.starts_with("src/") {
            out.extend(alias_problems(&path, &src));
        }
    }
    let cargo = read("Cargo.toml");
    out.extend(root_problems(&cargo, &read("clippy.toml")));
    for m in members(&cargo) {
        let path = if m == "." {
            "Cargo.toml".to_string()
        } else {
            format!("{m}/Cargo.toml")
        };
        out.extend(member_lint_problems(&path, &read(&path)));
    }
    for f in listed(root) {
        let name = f.rsplit('/').next().unwrap_or(&f);
        if (name == "clippy.toml" || name == ".clippy.toml") && f != "clippy.toml" {
            out.push(format!("{f}: only the repo root may have a clippy.toml"));
        }
        let config = f == ".cargo/config.toml" || f == ".cargo/config";
        let workflow = f.starts_with(".github/workflows/");
        if config || workflow {
            out.extend(flag_problems(&f, &read(&f)));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
