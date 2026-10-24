use std::path::Path;

use super::{
    Format, Loaded, Outcome, Role, detect_role, hide_secret_lines, json_syntax, schema_json,
};
use crate::config::check::{
    Spans, check_client, check_refs, check_server, check_visitor, secret_spans,
};
use crate::config::diagnostics::{Diagnostic, Report};
use crate::config::env::{EnvSource, expand};
use crate::config::schema::Checker;
use crate::config::tree::{self, Node, Span, Value};
use burrow_config::{ClientConfig, ServerConfig, VisitorConfig};

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
