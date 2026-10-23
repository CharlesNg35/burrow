use serde_json::{Map, Value as J};

use super::tree::{Entry, Node, Span, Value};
use crate::diagnostics::Diagnostic;
use crate::suggest::{closest, quoted_list};

pub struct Checker<'a> {
    root: &'a J,
    role: &'static str,
    pub diagnostics: Vec<Diagnostic>,
    pub prune: Vec<Vec<String>>,
    pub retype: Vec<(Vec<String>, Value)>,
}

#[derive(Clone)]
struct Ctx<'a> {
    path: Vec<std::borrow::Cow<'a, str>>,
    owner_span: Span,
}

impl<'a> Ctx<'a> {
    fn child(&self, key: impl Into<std::borrow::Cow<'a, str>>, span: Span) -> Ctx<'a> {
        let mut path = self.path.clone();
        path.push(key.into());
        Ctx {
            path,
            owner_span: span,
        }
    }

    fn field(&self) -> &str {
        self.path.last().map_or("", |s| s.as_ref())
    }

    fn parts(&self) -> Vec<&str> {
        self.path.iter().map(|s| s.as_ref()).collect()
    }

    fn path(&self) -> Vec<String> {
        self.path.iter().map(|s| s.to_string()).collect()
    }
}

pub fn owner_name(role: &str, path: &[&str]) -> String {
    match path {
        [] => format!("{role} config"),
        ["services", name] => format!("service \"{name}\""),
        ["clients", name] => format!("client \"{name}\""),
        ["visitors", name] => format!("visitor \"{name}\""),
        ["forwards", name] => format!("forward \"{name}\""),
        [section] => format!("\"{section}\""),
        [.., last] => format!("\"{last}\""),
    }
}

impl<'a> Checker<'a> {
    pub fn new(root: &'a J, role: &'static str) -> Self {
        Checker {
            root,
            role,
            diagnostics: Vec::new(),
            prune: Vec::new(),
            retype: Vec::new(),
        }
    }

    pub fn check(&mut self, node: &'a Node) {
        let ctx = Ctx {
            path: Vec::new(),
            owner_span: Span::point(node.span.start),
        };
        if !matches!(node.value, Value::Map(_)) {
            let msg = match node.value {
                Value::Null => "the config file is empty".to_string(),
                _ => format!(
                    "the config file must be a mapping of keys to values, found {}",
                    node.kind()
                ),
            };
            let mut d = Diagnostic::error("not-a-mapping", msg)
                .help(format!(
                    "start with \"listen: 0.0.0.0:{port}\" (server) or \"server: host:{port}\" (client), or run: {}",
                    crate::failure::cmd("init"),
                    port = burrow_syntax::ports::SERVER_PORT
                ));
            if node.span.start.line > 0 {
                d = d.at(node.span);
            }
            self.diagnostics.push(d);
            self.prune.push(Vec::new());
            return;
        }
        self.node(self.root, node, &ctx);
    }

    fn owner(&self, ctx: &Ctx<'a>) -> String {
        owner_name(self.role, &ctx.parts())
    }

    fn container(&self, ctx: &Ctx<'a>) -> String {
        let parts = ctx.parts();
        owner_name(self.role, &parts[..parts.len().saturating_sub(1)])
    }

    fn resolve(&self, schema: &'a J) -> &'a J {
        let mut s = schema;
        for _ in 0..16 {
            match s.get("$ref").and_then(J::as_str) {
                Some(r) => {
                    let name = r.trim_start_matches("#/$defs/");
                    match self.root.get("$defs").and_then(|d| d.get(name)) {
                        Some(next) => s = next,
                        None => return s,
                    }
                }
                None => return s,
            }
        }
        s
    }

    fn node(&mut self, schema: &'a J, node: &'a Node, ctx: &Ctx<'a>) {
        let schema = self.resolve(schema);
        if let Some(branches) = schema
            .get("anyOf")
            .or_else(|| schema.get("oneOf"))
            .and_then(J::as_array)
        {
            self.any_of(branches, node, ctx);
            return;
        }
        if let Some(values) = schema.get("enum").and_then(J::as_array) {
            self.enumeration(values, node, ctx);
            return;
        }
        let types = types_of(schema);
        if !types.is_empty() && !types.iter().any(|t| type_matches(t, node)) {
            if let Some(v) = from_env(&types, node) {
                if let Value::Int(i) = v {
                    self.range(schema, i, node, ctx);
                }
                self.retype.push((ctx.path(), v));
                return;
            }
            if unset(node) {
                self.prune.push(ctx.path());
                return;
            }
            self.type_error(&types, node, ctx, bounds_help(schema));
            return;
        }
        match &node.value {
            Value::Map(entries) => self.object(schema, entries, ctx),
            Value::Seq(items) => {
                if let Some(item) = schema.get("items") {
                    for (i, n) in items.iter().enumerate() {
                        self.node(item, n, &ctx.child(i.to_string(), n.span));
                    }
                }
            }
            Value::Int(i) => self.range(schema, *i, node, ctx),
            _ => {}
        }
    }

    fn flatten(&self, branches: &'a [J], out: &mut Vec<&'a J>) {
        for b in branches {
            let b = self.resolve(b);
            match b
                .get("anyOf")
                .or_else(|| b.get("oneOf"))
                .and_then(J::as_array)
            {
                Some(inner) => self.flatten(inner, out),
                None => out.push(b),
            }
        }
    }

    fn any_of(&mut self, branches: &'a [J], node: &'a Node, ctx: &Ctx<'a>) {
        let mut flat = Vec::new();
        self.flatten(branches, &mut flat);
        let branches = flat;
        let mut candidates = Vec::new();
        for &b in &branches {
            let types = types_of(b);
            if types.iter().any(|t| type_matches(t, node))
                || from_env(&types, node).is_some()
                || (types.is_empty()
                    && b.get("enum").is_some()
                    && matches!(node.value, Value::Str(_)))
            {
                candidates.push(b);
            }
        }
        if candidates.is_empty() && unset(node) {
            self.prune.push(ctx.path());
            return;
        }
        if candidates.is_empty() {
            let mut all: Vec<&str> = branches.iter().flat_map(|b| types_of(b)).collect();
            all.dedup();
            self.type_error(&all, node, ctx, None);
            return;
        }
        let mut best: Option<(Vec<Diagnostic>, Vec<Vec<String>>)> = None;
        for b in candidates {
            let saved = std::mem::take(&mut self.diagnostics);
            let saved_prune = std::mem::take(&mut self.prune);
            let saved_retype = self.retype.len();
            self.node(b, node, ctx);
            let found = std::mem::replace(&mut self.diagnostics, saved);
            let found_prune = std::mem::replace(&mut self.prune, saved_prune);
            if found.is_empty() {
                return;
            }
            self.retype.truncate(saved_retype);
            if best.as_ref().is_none_or(|(b, _)| found.len() < b.len()) {
                best = Some((found, found_prune));
            }
        }
        if let Some((d, p)) = best {
            self.diagnostics.extend(d);
            self.prune.extend(p);
        }
    }

    fn enumeration(&mut self, values: &'a [J], node: &'a Node, ctx: &Ctx<'a>) {
        let options: Vec<&str> = values.iter().filter_map(J::as_str).collect();
        let Some(given) = node.as_str() else {
            self.type_error(&["string"], node, ctx, None);
            return;
        };
        if options.contains(&given) {
            return;
        }
        let list = quoted_list(options.iter().copied());
        let help = match closest(given, options.iter().copied()) {
            Some(s) => format!("did you mean \"{s}\"? valid values: {list}"),
            None => format!("valid values: {list}"),
        };
        self.diagnostics.push(
            Diagnostic::error(
                "unknown-value",
                format!(
                    "unknown {} \"{given}\" in {}",
                    ctx.field(),
                    self.container(ctx)
                ),
            )
            .at(node.span)
            .label(format!("not a valid {}", ctx.field()))
            .help(help),
        );
        self.prune.push(ctx.path());
    }

    fn object(&mut self, schema: &'a J, entries: &'a [Entry], ctx: &Ctx<'a>) {
        let props = schema
            .get("properties")
            .and_then(J::as_object)
            .unwrap_or(&EMPTY);
        let additional = schema.get("additionalProperties");
        let prune_before = self.prune.len();
        for e in entries {
            let child = ctx.child(&e.key, e.key_span);
            if let Some(p) = props.get(&e.key) {
                self.node(p, &e.value, &child);
                continue;
            }
            match additional {
                Some(J::Bool(false)) => self.unknown_field(e, props, ctx),
                Some(s @ J::Object(_)) => self.node(s, &e.value, &child),
                _ => {}
            }
        }
        let required_names: Vec<&str> = schema
            .get("required")
            .and_then(J::as_array)
            .map(|r| r.iter().filter_map(J::as_str).collect())
            .unwrap_or_default();
        let own = ctx.path();
        let mut lost_required = false;
        for p in &mut self.prune[prune_before..] {
            if p.len() == own.len() + 1
                && p.starts_with(&own)
                && required_names.contains(&p[own.len()].as_str())
            {
                lost_required = true;
            }
        }
        if lost_required {
            self.prune.push(own);
        }
        if let Some(required) = schema.get("required").and_then(J::as_array) {
            for name in required.iter().filter_map(J::as_str) {
                if entries.iter().any(|e| e.key == name) {
                    continue;
                }
                let desc = props
                    .get(name)
                    .and_then(|p| {
                        p.get("description")
                            .or_else(|| self.resolve(p).get("description"))
                    })
                    .and_then(J::as_str);
                let help = match desc {
                    Some(d) => format!("add \"{name}: ...\" ({})", lowercase_first(d)),
                    None => format!("add \"{name}: ...\""),
                };
                let mut d = Diagnostic::error(
                    "missing-field",
                    format!("missing field \"{name}\" in {}", self.owner(ctx)),
                )
                .at(ctx.owner_span)
                .help(help);
                if ctx.path.is_empty() {
                    d = d.without_snippet();
                } else {
                    d = d.label(format!("\"{name}\" is required here"));
                }
                self.diagnostics.push(d);
                self.prune.push(ctx.path());
            }
        }
    }

    fn unknown_field(&mut self, e: &Entry, props: &'a Map<String, J>, ctx: &Ctx<'a>) {
        let names: Vec<&str> = props.keys().map(String::as_str).collect();
        let list = quoted_list(names.iter().copied());
        let help = match closest(&e.key, names.iter().copied()) {
            Some(s) => format!("did you mean \"{s}\"? valid fields: {list}"),
            None => format!("valid fields: {list}"),
        };
        self.diagnostics.push(
            Diagnostic::error(
                "unknown-field",
                format!("unknown field \"{}\" in {}", e.key, self.owner(ctx)),
            )
            .at(e.key_span)
            .label("unknown field")
            .help(help),
        );
        let mut path = ctx.path();
        path.push(e.key.clone());
        self.prune.push(path);
    }

    fn range(&mut self, schema: &J, value: i64, node: &Node, ctx: &Ctx<'a>) {
        let (fmt_min, fmt_max) = format_bounds(schema.get("format").and_then(J::as_str));
        let min = schema
            .get("minimum")
            .and_then(J::as_i64)
            .map_or(fmt_min, |m| m.max(fmt_min));
        let explicit_max = schema.get("maximum").and_then(J::as_i64);
        let max = explicit_max.map_or(fmt_max, |m| m.min(fmt_max));
        if value < min || value > max {
            let (bounds, help) = if explicit_max.is_none() && value < min {
                (format!("at least {min}"), format!("use {min} or more"))
            } else {
                (
                    format!("between {min} and {max}"),
                    bounds_help(schema)
                        .unwrap_or_else(|| format!("use a value between {min} and {max}")),
                )
            };
            self.diagnostics.push(
                Diagnostic::error(
                    "out-of-range",
                    format!(
                        "\"{}\" in {} must be {bounds}, found {value}",
                        ctx.field(),
                        self.container(ctx)
                    ),
                )
                .at(node.span)
                .label(format!("must be {bounds}"))
                .help(help),
            );
            self.prune.push(ctx.path());
        }
    }

    fn type_error(&mut self, types: &[&str], node: &Node, ctx: &Ctx<'a>, bounds: Option<String>) {
        let want = types
            .iter()
            .filter(|t| **t != "null")
            .map(|t| type_name(t))
            .collect::<Vec<_>>()
            .join(" or ");
        let help = match (ctx.field(), &node.value) {
            ("reach", Value::Seq(_)) => {
                "write it as a mapping of client names, for example: reach: { home: any }".into()
            }
            _ => type_help(types, node, bounds),
        };
        self.diagnostics.push(
            Diagnostic::error(
                "wrong-type",
                format!(
                    "\"{}\" in {} must be {want}, found {}",
                    ctx.field(),
                    self.container(ctx),
                    node.kind()
                ),
            )
            .at(node.span)
            .label(format!("expected {want}"))
            .help(help),
        );
        self.prune.push(ctx.path());
    }
}

static EMPTY: std::sync::LazyLock<Map<String, J>> = std::sync::LazyLock::new(Map::new);

fn types_of(schema: &J) -> Vec<&str> {
    match schema.get("type") {
        Some(J::String(s)) => vec![s.as_str()],
        Some(J::Array(a)) => a.iter().filter_map(J::as_str).collect(),
        _ => Vec::new(),
    }
}

fn from_env(types: &[&str], node: &Node) -> Option<Value> {
    let (crate::config::tree::FromEnv::Expanded, Value::Str(s)) = (node.env, &node.value) else {
        return None;
    };
    let s = s.trim();
    if (types.contains(&"integer") || types.contains(&"number"))
        && let Ok(i) = s.parse::<i64>()
    {
        return Some(Value::Int(i));
    }
    if types.contains(&"number")
        && let Ok(f) = s.parse::<f64>()
    {
        return Some(Value::Float(f));
    }
    if types.contains(&"boolean") {
        return match s {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        };
    }
    None
}

fn unset(node: &Node) -> bool {
    node.env == crate::config::tree::FromEnv::Unset
}

fn type_matches(t: &str, node: &Node) -> bool {
    matches!(
        (t, &node.value),
        ("string", Value::Str(_))
            | ("integer", Value::Int(_))
            | ("number", Value::Int(_) | Value::Float(_))
            | ("boolean", Value::Bool(_))
            | ("array", Value::Seq(_))
            | ("object", Value::Map(_))
            | ("null", Value::Null)
    )
}

fn type_name(t: &str) -> &'static str {
    match t {
        "string" => "a string",
        "integer" => "a whole number",
        "number" => "a number",
        "boolean" => "true or false",
        "array" => "a list",
        "object" => "a mapping",
        _ => "null",
    }
}

fn type_help(types: &[&str], node: &Node, bounds: Option<String>) -> String {
    let quoted = node.quoted;
    match (&node.value, types) {
        (Value::Str(s), t) if t.contains(&"integer") && quoted && s.parse::<i64>().is_ok() => {
            format!("remove the quotes: {s}")
        }
        (Value::Str(s), t)
            if t.contains(&"boolean")
                && matches!(s.as_str(), "yes" | "no" | "on" | "off" | "y" | "n") =>
        {
            "use true or false".into()
        }
        (Value::Str(s), t)
            if t.contains(&"boolean") && quoted && matches!(s.as_str(), "true" | "false") =>
        {
            format!("remove the quotes: {s}")
        }
        (Value::Float(f), t) if t.contains(&"integer") && f.fract() == 0.0 && f.abs() < 9.0e15 => {
            format!(
                "write it without a decimal point or exponent: {}",
                *f as i64
            )
        }
        (Value::Float(f), t) if t.contains(&"integer") && f.fract() == 0.0 => {
            "this number is far too large for the field".into()
        }
        (Value::Null, _) => "give it a value, or remove the line".into(),
        (Value::Seq(_), t) if !t.contains(&"array") => "use a single value, not a list".into(),
        (Value::Map(_), t) if !t.contains(&"object") => "use a single value, not a mapping".into(),
        (_, t) if t.contains(&"object") => {
            "write it as an indented block of \"key: value\" lines".into()
        }
        (_, t) if t.contains(&"array") => {
            "write it as a list, for example: [a, b] or lines starting with \"- \"".into()
        }
        (_, t) if t.contains(&"string") => "wrap the value in quotes".into(),
        _ => bounds.unwrap_or_else(|| {
            format!(
                "expected {}",
                types
                    .iter()
                    .map(|t| type_name(t))
                    .collect::<Vec<_>>()
                    .join(" or ")
            )
        }),
    }
}

fn bounds_help(schema: &J) -> Option<String> {
    schema.get("x-show-default")?;
    let min = schema.get("minimum")?.as_i64()?;
    let max = schema.get("maximum")?.as_i64()?;
    let default = schema.get("default")?.as_i64()?;
    Some(format!(
        "use a value between {min} and {max}; the default is {default}"
    ))
}

fn format_bounds(format: Option<&str>) -> (i64, i64) {
    match format {
        Some("uint8") => (0, u8::MAX.into()),
        Some("uint16") => (0, u16::MAX.into()),
        Some("uint32" | "uint") => (0, u32::MAX.into()),
        Some("uint64") => (0, i64::MAX),
        Some("int32") => (i32::MIN.into(), i32::MAX.into()),
        _ => (i64::MIN, i64::MAX),
    }
}

fn lowercase_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}
