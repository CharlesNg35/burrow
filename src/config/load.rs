use std::path::Path;

use super::check::{Spans, check_client, check_refs, check_server, check_visitor, secret_spans};
use super::env::{EnvSource, expand};
use super::model::{ClientConfig, ServerConfig, VisitorConfig};
use super::schema::Checker;
use super::settings::{ClientSettings, ServerSettings, VisitorSettings};
use super::tree::{self, Node, Pos, Span, Value};
use crate::diagnostics::{Diagnostic, Report};

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
                    crate::failure::command()
                ),
            ),
            LoadError::Read(e) if e.kind() == std::io::ErrorKind::PermissionDenied => (
                format!("cannot read {}: permission denied", path.display()),
                "run as the file's owner, or check its permissions with: ls -l".into(),
            ),
            LoadError::Read(e) if e.kind() == std::io::ErrorKind::InvalidData => (
                format!("{} is not valid UTF-8 text", path.display()),
                "save the file as UTF-8".into(),
            ),
            LoadError::Read(e) => (
                format!("cannot read {}: {e}", path.display()),
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
    let src = std::fs::read_to_string(path).map_err(LoadError::Read)?;
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut outcome = load_str_in(base, &path.display().to_string(), &src, format, role, env);
    if let Some(w) = permission_warning(path) {
        outcome.report.push(w);
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
        .help(format!("restrict it with: chmod 600 {}", path.display()))
    })
}

#[cfg(not(unix))]
fn permission_warning(_: &Path) -> Option<Diagnostic> {
    None
}

pub fn detect_role(root: &Node) -> Option<Role> {
    let Value::Map(entries) = &root.value else {
        return None;
    };
    if entries.iter().any(|e| e.key == "forwards") {
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
                report.push(
                    Diagnostic::error("syntax", e.message)
                        .at(e.span)
                        .label("here")
                        .help("check the indentation and the \"key: value\" structure around this line"),
                );
            }
            hide_secret_lines(&mut report);
            report.mask_tokens = true;
            return fail(report, role);
        }
    };
    report.masked = secret_spans(&root);

    let role = match role.or_else(|| detect_role(&root)) {
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
    let structural_ok = checker.diagnostics.is_empty();
    let prune = std::mem::take(&mut checker.prune);
    checker.diagnostics.into_iter().for_each(|d| report.push(d));

    let mut pruned = root.clone();
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
                Err(e) => sem.push(internal(e)),
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
            ("secret", text(crate::secret::Secret::generate().encode())),
        ]
    };
    let placeholders: Vec<(&str, Value)> = match role {
        Role::Server => vec![],
        Role::Client => connection(),
        Role::Visitor => {
            let mut v = connection();
            v.push(("forwards", Value::Map(Vec::new())));
            v
        }
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
