use std::path::Path;

use super::check::{Spans, check_client, check_refs, check_server, check_visitor, secret_spans};
use super::env::{EnvSource, expand};
use super::schema::Checker;
use super::settings::{ClientSettings, ServerSettings, VisitorSettings};
use super::tree::{self, Node, Pos, Span, Value};
use crate::config::diagnostics::{Diagnostic, Report};
use burrow_config::{ClientConfig, ServerConfig, VisitorConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Server,
    Client,
    Visitor,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Server => "server",
            Role::Client => "client",
            Role::Visitor => "visitor",
        }
    }

    pub fn parse(word: &str) -> Option<Role> {
        [Role::Server, Role::Client, Role::Visitor]
            .into_iter()
            .find(|r| r.as_str() == word)
    }

    pub fn command(self) -> &'static str {
        match self {
            Role::Visitor => "visit",
            r => r.as_str(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Yaml,
    Json,
}

impl Format {
    pub fn from_path(path: &Path) -> Option<Format> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "yaml" | "yml" => Some(Format::Yaml),
            "json" => Some(Format::Json),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Loaded {
    Server(Box<ServerSettings>, Box<ServerConfig>),
    Client(Box<ClientSettings>, Box<ClientConfig>),
    Visitor(Box<VisitorSettings>, Box<VisitorConfig>),
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub report: Report,
    pub role: Option<Role>,
    pub loaded: Option<Loaded>,
}

#[derive(Debug)]
pub enum LoadError {
    Read(std::io::Error),
    UnknownExtension,
}

impl LoadError {
    pub fn describe(&self, path: &Path) -> (String, String) {
        match self {
            LoadError::Read(e) if e.kind() == std::io::ErrorKind::NotFound => (
                format!("config file {} does not exist", path.display()),
                format!(
                    "check the path, or create one with: {0} init server  (or: {0} init client, {0} init visitor)",
                    crate::base::failure::command()
                ),
            ),
            LoadError::Read(e) if e.kind() == std::io::ErrorKind::PermissionDenied => (
                format!("cannot read {}: permission denied", path.display()),
                if cfg!(windows) {
                    format!(
                        "run as an administrator, or check its permissions with: icacls {}",
                        crate::base::failure::path_arg(path)
                    )
                } else {
                    format!(
                        "run as the file's owner, or check its permissions with: ls -l {}",
                        crate::base::failure::path_arg(path)
                    )
                },
            ),
            LoadError::Read(e) if e.kind() == std::io::ErrorKind::InvalidData => (
                format!("{} is not valid UTF-8 text", path.display()),
                "save the file as UTF-8".into(),
            ),
            LoadError::Read(e) => (
                format!(
                    "cannot read {}: {}",
                    path.display(),
                    crate::base::failure::file_reason(e)
                ),
                "check that the path is a readable file".into(),
            ),
            LoadError::UnknownExtension => (
                format!("cannot tell the format of {}", path.display()),
                "name the file with a .yaml, .yml or .json extension".into(),
            ),
        }
    }
}

pub fn load_file(
    path: &Path,
    role: Option<Role>,
    env: &dyn EnvSource,
) -> Result<Outcome, LoadError> {
    let format = Format::from_path(path).ok_or(LoadError::UnknownExtension)?;
    let meta = std::fs::metadata(path).map_err(LoadError::Read)?;
    if !meta.is_file() && !meta.is_dir() {
        return Err(LoadError::Read(std::io::Error::other(
            "it is not a regular file",
        )));
    }
    let src = std::fs::read_to_string(path).map_err(LoadError::Read)?;
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let named = path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.to_ascii_lowercase().ends_with("visitor"));
    let role = role.or_else(|| named.then(|| visitor_by_name(&src)).flatten());
    let mut outcome = load_str_in(base, &path.display().to_string(), &src, format, role, env);
    if let Some(w) = permission_warning(path) {
        outcome.report.push(w);
        outcome.report.sort();
    }
    if outcome
        .report
        .diagnostics
        .iter()
        .any(|d| d.code == "syntax")
        && let Some(spare) = crate::config::save::spare_next_to(path)
    {
        outcome.report.push(
            Diagnostic::warning(
                "edit-spare",
                format!(
                    "{} holds the text of a burrow edit save that did not finish",
                    spare.display()
                ),
            )
            .help("if this file was cut short while saving, copy that file over it"),
        );
        outcome.report.sort();
    }
    Ok(outcome)
}

#[cfg(unix)]
fn permission_warning(path: &Path) -> Option<Diagnostic> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok()?.permissions().mode();
    (mode & 0o004 != 0).then(|| {
        Diagnostic::warning(
            "world-readable",
            format!(
                "config file is readable by every user on this machine (mode {:o})",
                mode & 0o777
            ),
        )
        .help(format!(
            "restrict it with: chmod 600 {}",
            crate::base::failure::path_arg(path)
        ))
    })
}

#[cfg(not(unix))]
fn permission_warning(_: &Path) -> Option<Diagnostic> {
    None
}

fn visitor_by_name(src: &str) -> Option<Role> {
    let root = tree::parse(src).ok()?;
    let says_client = match &root.value {
        Value::Map(entries) => entries
            .iter()
            .any(|e| e.key == "allow" || e.key == "allow_all"),
        _ => false,
    };
    (detect_role(&root) != Some(Role::Server) && !says_client).then_some(Role::Visitor)
}

pub fn detect_role(root: &Node) -> Option<Role> {
    let Value::Map(entries) = &root.value else {
        return None;
    };
    if entries
        .iter()
        .any(|e| e.key == "forwards" || e.key == "forward")
    {
        return Some(Role::Visitor);
    }
    let server = entries
        .iter()
        .filter(|e| {
            matches!(
                e.key.as_str(),
                "listen"
                    | "websocket"
                    | "clients"
                    | "visitors"
                    | "services"
                    | "limits"
                    | "pool"
                    | "heartbeat"
            )
        })
        .count();
    let client = entries
        .iter()
        .filter(|e| {
            matches!(
                e.key.as_str(),
                "server"
                    | "name"
                    | "secret"
                    | "allow"
                    | "allow_all"
                    | "proxy"
                    | "reconnect"
                    | "cipher"
            )
        })
        .count();
    match server.cmp(&client) {
        std::cmp::Ordering::Greater => Some(Role::Server),
        std::cmp::Ordering::Less => Some(Role::Client),
        std::cmp::Ordering::Equal => None,
    }
}

pub fn schema_json(role: Role) -> serde_json::Value {
    let schema = match role {
        Role::Server => schemars::schema_for!(ServerConfig),
        Role::Client => schemars::schema_for!(ClientConfig),
        Role::Visitor => schemars::schema_for!(VisitorConfig),
    };
    serde_json::to_value(schema).expect("schema serializes")
}

fn json_syntax(src: &str) -> Option<Diagnostic> {
    let err = serde_json::from_str::<serde::de::IgnoredAny>(src).err()?;
    let line = err.line();
    let col = err.column().max(1);
    let offset = src
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum::<usize>()
        + col
        - 1;
    let pos = Pos { offset, line, col };
    let msg = err.to_string();
    let msg = msg.split(" at line ").next().unwrap_or(&msg).to_string();
    Some(
        Diagnostic::error("syntax", format!("invalid JSON: {msg}"))
            .at(Span::point(pos))
            .label("here")
            .help("check for a missing comma, quote or bracket just before this point"),
    )
}

fn hide_secret_lines(report: &mut Report) {
    let lines: Vec<&str> = report.source.lines().collect();
    for d in &mut report.diagnostics {
        if let Some(s) = d.span {
            let line = lines
                .get(s.start.line.saturating_sub(1))
                .copied()
                .unwrap_or("");
            if line.contains("secret") {
                d.snippet = false;
            }
        }
    }
}

pub fn load_str(
    name: &str,
    src: &str,
    format: Format,
    role: Option<Role>,
    env: &dyn EnvSource,
) -> Outcome {
    load_str_in(Path::new("."), name, src, format, role, env)
}

fn unquoted_variable_in_flow(line: &str) -> bool {
    let unquoted = line
        .match_indices("${")
        .any(|(i, _)| !line[..i].ends_with(['"', '\'']));
    unquoted && line.replace("${", "").contains(['{', '['])
}

pub fn load_str_in(
    base_dir: &Path,
    name: &str,
    src: &str,
    format: Format,
    role: Option<Role>,
    env: &dyn EnvSource,
) -> Outcome {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut report = Report::new(name, src);
    let fail = |mut report: Report, role| {
        report.sort();
        Outcome {
            report,
            role,
            loaded: None,
        }
    };

    if format == Format::Json
        && let Some(d) = json_syntax(src)
    {
        report.push(d);
        hide_secret_lines(&mut report);
        report.mask_tokens = true;
        return fail(report, role);
    }
    let mut root = match tree::parse(src) {
        Ok(n) => n,
        Err(errors) => {
            for e in errors {
                let line = src
                    .lines()
                    .nth(e.span.start.line.saturating_sub(1))
                    .unwrap_or("");
                let help = if unquoted_variable_in_flow(line) {
                    "a ${VAR} inside { ... } must be quoted, like \"${VAR}\""
                } else {
                    "check the indentation and the \"key: value\" structure around this line"
                };
                report.push(
                    Diagnostic::error("syntax", e.message)
                        .at(e.span)
                        .label("here")
                        .help(help),
                );
            }
            hide_secret_lines(&mut report);
            report.mask_tokens = true;
            return fail(report, role);
        }
    };
    report.masked = secret_spans(&root);

    let detected = detect_role(&root);
    if let (Some(wanted), Some(found)) = (role, detected) {
        let clear = match (wanted, found) {
            (Role::Server, other) => other != Role::Server,
            (_, Role::Server) => true,
            (Role::Client, Role::Visitor) => true,
            _ => false,
        };
        if clear {
            let run = match found {
                Role::Server => "server",
                Role::Client => "client",
                Role::Visitor => "visit",
            };
            report.push(
                Diagnostic::error(
                    "wrong-role",
                    format!(
                        "{name} is a {} config, not a {} config",
                        found.as_str(),
                        wanted.as_str()
                    ),
                )
                .without_snippet()
                .help(format!(
                    "run it with: {}",
                    crate::base::failure::cmd(&format!(
                        "{run} -c {}",
                        crate::base::failure::arg(name)
                    ))
                )),
            );
            return fail(report, Some(wanted));
        }
    }
    let role = match role.or(detected) {
        Some(r) => r,
        None => {
            if matches!(root.value, Value::Map(_)) {
                report.push(
                    Diagnostic::error("unknown-role", "cannot tell whether this is a server, client or visitor config")
                        .without_snippet()
                        .help("a server config has \"listen\" and \"services\", a client config has \"server\" and \"name\", a visitor config also has \"forwards\"; or pass --role server|client|visitor"),
                );
                return fail(report, None);
            }
            Role::Server
        }
    };

    let mut env_diags = Vec::new();
    expand(&mut root, src, env, &mut env_diags);
    let env_spans: Vec<Span> = env_diags.iter().filter_map(|d| d.span).collect();
    env_diags.into_iter().for_each(|d| report.push(d));

    let schema = schema_json(role);
    let mut checker = Checker::new(&schema, role.as_str());
    checker.check(&root);
    let structural_ok = checker.diagnostics.is_empty() && checker.prune.is_empty();
    let prune = std::mem::take(&mut checker.prune);
    let retype = std::mem::take(&mut checker.retype);
    checker.diagnostics.into_iter().for_each(|d| report.push(d));

    let mut pruned = root.clone();
    for (path, value) in retype {
        set_path(&mut pruned, &path, value);
    }
    let mut semantic = matches!(root.value, Value::Map(_));
    if !structural_ok {
        for path in &prune {
            if path.is_empty() {
                fill_root_placeholders(&mut pruned, role);
            } else {
                remove_path(&mut pruned, path);
            }
        }
        drop_pruned_items(&mut pruned);
        if !matches!(pruned.value, Value::Map(_)) {
            semantic = false;
        }
    }

    if role == Role::Server {
        let mut refs = Vec::new();
        check_refs(&root, &mut refs);
        refs.into_iter().for_each(|d| report.push(d));
    }

    let mut loaded = None;
    if semantic {
        let spans = Spans::new(&root);
        let mut sem = Vec::new();
        let json = pruned.to_json();
        match role {
            Role::Server => match serde_json::from_value::<ServerConfig>(json) {
                Ok(cfg) => {
                    if let Some(s) = check_server(&cfg, &spans, base_dir, &mut sem) {
                        loaded = Some(Loaded::Server(Box::new(s), Box::new(cfg)));
                    }
                }
                Err(e) => {
                    if structural_ok {
                        sem.push(internal(e));
                    }
                }
            },
            Role::Client => match serde_json::from_value::<ClientConfig>(json) {
                Ok(cfg) => {
                    if let Some(s) = check_client(&cfg, &spans, base_dir, &mut sem) {
                        loaded = Some(Loaded::Client(Box::new(s), Box::new(cfg)));
                    }
                }
                Err(e) => {
                    if structural_ok {
                        sem.push(internal(e));
                    }
                }
            },
            Role::Visitor => match serde_json::from_value::<VisitorConfig>(json) {
                Ok(cfg) => {
                    if let Some(s) = check_visitor(&cfg, &spans, base_dir, &mut sem) {
                        loaded = Some(Loaded::Visitor(Box::new(s), Box::new(cfg)));
                    }
                }
                Err(e) => {
                    if structural_ok {
                        sem.push(internal(e));
                    }
                }
            },
        }
        let raw_len = |key: &str| match root.get(key).map(|n| &n.value) {
            Some(Value::Map(m)) => m.len(),
            _ => 0,
        };
        for d in sem {
            let hidden = d
                .span
                .is_some_and(|s| env_spans.iter().any(|e| within(s, *e)))
                || (d.code == "no-services" && raw_len("services") > 0)
                || (d.code == "no-clients" && raw_len("clients") > 0);
            if !hidden {
                report.push(d);
            }
        }
    }

    if report.has_errors() {
        loaded = None;
    }
    report.sort();
    Outcome {
        report,
        role: Some(role),
        loaded,
    }
}

fn fill_root_placeholders(node: &mut Node, role: Role) {
    let Value::Map(entries) = &mut node.value else {
        return;
    };
    let text = |s: String| Value::Str(s);
    let connection = || {
        vec![
            ("server", text("placeholder.invalid:1".into())),
            ("name", text("placeholder".into())),
            (
                "secret",
                text(crate::wire::secret::Secret::generate().encode()),
            ),
        ]
    };
    let placeholders: Vec<(&str, Value)> = match role {
        Role::Server => vec![],
        Role::Client => connection(),
        Role::Visitor => connection(),
    };
    for (key, value) in placeholders {
        if !entries.iter().any(|e| e.key == key) {
            entries.push(tree::Entry {
                key: key.to_string(),
                key_span: Span::default(),
                value: Node {
                    value,
                    span: Span::default(),
                    quoted: true,
                    env: tree::FromEnv::No,
                },
            });
        }
    }
}

fn internal(e: serde_json::Error) -> Diagnostic {
    Diagnostic::error("invalid", format!("config could not be loaded: {e}"))
        .without_snippet()
        .help("this is likely a burrow bug; please report it with the config (secrets removed)")
}

fn within(a: Span, b: Span) -> bool {
    a.start.line == b.start.line
        || (a.start.offset <= b.start.offset && b.end.offset <= a.end.offset)
}

fn set_path(node: &mut Node, path: &[String], value: Value) {
    let Some((first, rest)) = path.split_first() else {
        node.value = value;
        return;
    };
    let next = match &mut node.value {
        Value::Map(entries) => entries
            .iter_mut()
            .find(|e| &e.key == first)
            .map(|e| &mut e.value),
        Value::Seq(items) => first.parse::<usize>().ok().and_then(|i| items.get_mut(i)),
        _ => None,
    };
    if let Some(next) = next {
        set_path(next, rest, value);
    }
}

fn remove_path(node: &mut Node, path: &[String]) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    match &mut node.value {
        Value::Map(entries) => {
            if rest.is_empty() {
                entries.retain(|e| &e.key != first);
            } else if let Some(e) = entries.iter_mut().find(|e| &e.key == first) {
                remove_path(&mut e.value, rest);
            }
        }
        Value::Seq(items) => {
            if let Ok(i) = first.parse::<usize>() {
                if rest.is_empty() {
                    if i < items.len() {
                        items[i].value = Value::Null;
                    }
                } else if let Some(item) = items.get_mut(i) {
                    remove_path(item, rest);
                }
            }
        }
        _ => {}
    }
}

fn drop_pruned_items(node: &mut Node) {
    match &mut node.value {
        Value::Seq(items) => {
            items.retain(|n| n.value != Value::Null);
            items.iter_mut().for_each(drop_pruned_items);
        }
        Value::Map(entries) => entries
            .iter_mut()
            .for_each(|e| drop_pruned_items(&mut e.value)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broken_config_names_a_spare_left_by_burrow_edit() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("server.yaml");
        std::fs::write(&config, "listen: [7835\n").unwrap();
        let nothing = |_: &str| None;
        let codes = |o: &Outcome| {
            o.report
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>()
        };
        let before = load_file(&config, Some(Role::Server), &nothing).unwrap();
        assert!(codes(&before).contains(&"syntax"));
        assert!(!codes(&before).contains(&"edit-spare"));
        let spare = dir.path().join(".server.yaml.burrow-edit-9");
        std::fs::write(&spare, "listen: 7835\n").unwrap();
        let after = load_file(&config, Some(Role::Server), &nothing).unwrap();
        let hint = after
            .report
            .diagnostics
            .iter()
            .find(|d| d.code == "edit-spare")
            .expect("the spare is named");
        assert!(
            hint.message.contains(".server.yaml.burrow-edit-9"),
            "{}",
            hint.message
        );
        std::fs::write(&config, "listen: 7835\n").unwrap();
        let fine = load_file(&config, Some(Role::Server), &nothing).unwrap();
        assert!(!codes(&fine).contains(&"edit-spare"));
    }
}
