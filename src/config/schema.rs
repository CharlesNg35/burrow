use serde_json::{Map, Value as J};

use super::tree::{Entry, Node, Span, Value};
use crate::diagnostics::Diagnostic;
use crate::suggest::{closest, quoted_list};

pub struct Checker<'a> {
    root: &'a J,
    role: &'static str,
    pub diagnostics: Vec<Diagnostic>,
    pub prune: Vec<Vec<String>>,
}

#[derive(Clone)]
struct Ctx<'a> {
    path: Vec<&'a str>,
    owner_span: Span,
}

impl<'a> Ctx<'a> {
    fn child(&self, key: &'a str, span: Span) -> Ctx<'a> {
        let mut path = self.path.clone();
        path.push(key);
        Ctx {
            path,
            owner_span: span,
        }
    }

    fn field(&self) -> &str {
        self.path.last().copied().unwrap_or("")
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
                    "start with \"listen: 0.0.0.0:7835\" (server) or \"server: host:7835\" (client), or run: {}",
                    crate::failure::cmd("init")
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
        owner_name(self.role, &ctx.path)
    }

    fn container(&self, ctx: &Ctx<'a>) -> String {
        owner_name(self.role, &ctx.path[..ctx.path.len().saturating_sub(1)])
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
            self.type_error(&types, node, ctx);
            return;
        }
        match &node.value {
            Value::Map(entries) => self.object(schema, entries, ctx),
            Value::Seq(items) => {
                if let Some(item) = schema.get("items") {
                    for (i, n) in items.iter().enumerate() {
                        let key: &'a str = INDEX_NAMES.get(i).copied().unwrap_or("item");
                        self.node(item, n, &ctx.child(key, n.span));
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
                || (types.is_empty()
                    && b.get("enum").is_some()
                    && matches!(node.value, Value::Str(_)))
            {
                candidates.push(b);
            }
        }
        if candidates.is_empty() {
            let mut all: Vec<&str> = branches.iter().flat_map(|b| types_of(b)).collect();
            all.dedup();
            self.type_error(&all, node, ctx);
            return;
        }
        let mut best: Option<(Vec<Diagnostic>, Vec<Vec<String>>)> = None;
        for b in candidates {
            let saved = std::mem::take(&mut self.diagnostics);
            let saved_prune = std::mem::take(&mut self.prune);
            self.node(b, node, ctx);
            let found = std::mem::replace(&mut self.diagnostics, saved);
            let found_prune = std::mem::replace(&mut self.prune, saved_prune);
            if found.is_empty() {
                return;
            }
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
            self.type_error(&["string"], node, ctx);
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
        if let [.., "reach", client] = ctx.path.as_slice() {
            self.diagnostics.push(
                Diagnostic::error(
                    "unknown-value",
                    format!("invalid reach \"{given}\" for client \"{client}\""),
                )
                .at(node.span)
                .label("expected any")
                .help(format!("the only value is any: {client}: any")),
            );
            self.prune.push(ctx.path());
            return;
        }
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
                    format!("use a value between {min} and {max}"),
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

    fn type_error(&mut self, types: &[&str], node: &Node, ctx: &Ctx<'a>) {
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
            _ => type_help(types, node),
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

const INDEX_NAMES: [&str; 64] = [
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16",
    "17", "18", "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31", "32",
    "33", "34", "35", "36", "37", "38", "39", "40", "41", "42", "43", "44", "45", "46", "47", "48",
    "49", "50", "51", "52", "53", "54", "55", "56", "57", "58", "59", "60", "61", "62", "63",
];

fn types_of(schema: &J) -> Vec<&str> {
    match schema.get("type") {
        Some(J::String(s)) => vec![s.as_str()],
        Some(J::Array(a)) => a.iter().filter_map(J::as_str).collect(),
        _ => Vec::new(),
    }
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

fn type_help(types: &[&str], node: &Node) -> String {
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
        _ => format!(
            "expected {}",
            types
                .iter()
                .map(|t| type_name(t))
                .collect::<Vec<_>>()
                .join(" or ")
        ),
    }
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
