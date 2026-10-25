use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    Base,
    Wire,
    Config,
    Roles,
    Ops,
}

pub const GROUPS: &[&str] = &["app", "base", "policy", "wire"];

pub const UNITS: &[(&str, Layer)] = &[
    ("base::access", Layer::Base),
    ("base::failure", Layer::Base),
    ("base::fs", Layer::Base),
    ("base::logfile", Layer::Base),
    ("base::ports", Layer::Base),
    ("base::process", Layer::Base),
    ("base::prompt", Layer::Base),
    ("base::style", Layer::Base),
    ("base::suggest", Layer::Base),
    ("wire::handshake", Layer::Wire),
    ("wire::noise", Layer::Wire),
    ("wire::protocol", Layer::Wire),
    ("wire::proxy", Layer::Wire),
    ("wire::proxy_protocol", Layer::Wire),
    ("wire::record", Layer::Wire),
    ("wire::secret", Layer::Wire),
    ("wire::transport", Layer::Wire),
    ("config", Layer::Config),
    ("install", Layer::Config),
    ("metrics", Layer::Config),
    ("policy::allowlist", Layer::Config),
    ("policy::ratelimit", Layer::Config),
    ("policy::throttle", Layer::Config),
    ("client", Layer::Roles),
    ("dialer", Layer::Roles),
    ("forward", Layer::Roles),
    ("server", Layer::Roles),
    ("udp", Layer::Roles),
    ("visitor", Layer::Roles),
    ("admin", Layer::Ops),
    ("admin_link", Layer::Ops),
    ("admin_local", Layer::Ops),
    ("app::command", Layer::Ops),
    ("app::cli", Layer::Ops),
    ("app::reload", Layer::Ops),
    ("app::run", Layer::Ops),
    ("edit", Layer::Ops),
    ("init", Layer::Ops),
    ("instance", Layer::Ops),
    ("service", Layer::Ops),
    ("status", Layer::Ops),
    ("update", Layer::Ops),
];

pub const MACROS: &[(&str, &str)] = &[("say_err", "base::failure"), ("say_errln", "base::failure")];

pub const GUARDS: &[(&str, &str, &str)] = &[(
    "src/wire/noise.rs",
    "pub struct InMemory(pub(in crate::wire) ());",
    "only code under src/wire may make an unencrypted in-memory session",
)];

pub fn layer_of(unit: &str) -> Option<Layer> {
    UNITS.iter().find(|(u, _)| *u == unit).map(|(_, l)| *l)
}

pub fn module_of(rel: &str) -> Option<Vec<String>> {
    let inner = rel.strip_prefix("src/")?.strip_suffix(".rs")?;
    let mut segs: Vec<String> = inner.split('/').map(str::to_string).collect();
    if segs.last().is_some_and(|s| s == "mod") || segs == ["lib"] {
        segs.pop();
    }
    Some(segs)
}

pub fn unit_of(module: &[String]) -> Option<String> {
    let first = module.first()?;
    if let Some((_, unit)) = MACROS.iter().find(|(m, _)| m == first) {
        return Some(unit.to_string());
    }
    if GROUPS.contains(&first.as_str()) {
        return module.get(1).map(|second| format!("{first}::{second}"));
    }
    Some(first.clone())
}

#[derive(Debug, Default)]
pub struct Scan {
    pub targets: Vec<Vec<String>>,
    pub banned: Vec<String>,
}

fn is_path_sep(tokens: &[TokenTree], i: usize) -> bool {
    matches!(
        (tokens.get(i), tokens.get(i + 1)),
        (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)))
            if a.as_char() == ':' && a.spacing() == Spacing::Joint && b.as_char() == ':'
    )
}

fn split_items(stream: TokenStream) -> Vec<Vec<TokenTree>> {
    let mut items = vec![Vec::new()];
    for t in stream {
        match &t {
            TokenTree::Punct(p) if p.as_char() == ',' => items.push(Vec::new()),
            _ => items.last_mut().expect("an item").push(t),
        }
    }
    items.retain(|i| !i.is_empty());
    items
}

fn chain(tokens: &[TokenTree], mut i: usize, prefix: Vec<String>, out: &mut Vec<Vec<String>>) {
    let mut segs = prefix;
    loop {
        match tokens.get(i) {
            Some(TokenTree::Ident(id)) if id == "super" => {
                segs.pop();
                i += 1;
            }
            Some(TokenTree::Ident(id)) if id == "self" => i += 1,
            Some(TokenTree::Ident(id)) => {
                segs.push(id.to_string());
                i += 1;
            }
            Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                for item in split_items(g.stream()) {
                    chain(&item, 0, segs.clone(), out);
                }
                return;
            }
            _ => break,
        }
        if is_path_sep(tokens, i) {
            i += 2;
        } else {
            break;
        }
    }
    out.push(segs);
}

fn after_in(tokens: &[TokenTree], i: usize) -> bool {
    i > 0 && matches!(&tokens[i - 1], TokenTree::Ident(id) if id == "in")
}

fn inside_path(tokens: &[TokenTree], i: usize) -> bool {
    const BEFORE_A_PATH: &[&str] = &[
        "use", "pub", "return", "let", "in", "as", "dyn", "impl", "for",
    ];
    i >= 3
        && is_path_sep(tokens, i - 2)
        && matches!(&tokens[i - 3], TokenTree::Ident(id) if !BEFORE_A_PATH.contains(&id.to_string().as_str()))
}

fn bracket_starts_with(t: Option<&TokenTree>, word: &str) -> bool {
    match t {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket => g
            .stream()
            .into_iter()
            .next()
            .is_some_and(|f| matches!(&f, TokenTree::Ident(id) if id == word)),
        _ => false,
    }
}

fn scan_stream(stream: TokenStream, module: &[String], scan: &mut Scan) {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            TokenTree::Ident(id) if id == "mod" => {
                if let (Some(TokenTree::Ident(name)), Some(TokenTree::Group(g))) =
                    (tokens.get(i + 1), tokens.get(i + 2))
                    && g.delimiter() == Delimiter::Brace
                {
                    let mut inner = module.to_vec();
                    inner.push(name.to_string());
                    scan_stream(g.stream(), &inner, scan);
                    i += 3;
                    continue;
                }
            }
            TokenTree::Ident(id)
                if (id == "crate" || id == "super" || id == "self" || id == "burrow")
                    && is_path_sep(&tokens, i + 1)
                    && !after_in(&tokens, i)
                    && !(id == "burrow" && inside_path(&tokens, i)) =>
            {
                let start = match id.to_string().as_str() {
                    "crate" | "burrow" => Vec::new(),
                    "super" => module[..module.len().saturating_sub(1)].to_vec(),
                    _ => module.to_vec(),
                };
                let mut found = Vec::new();
                chain(&tokens, i + 3, start, &mut found);
                scan.targets.extend(found);
                i += 3;
                continue;
            }
            TokenTree::Ident(id)
                if id == "include" || id == "include_str" || id == "include_bytes" =>
            {
                if matches!(tokens.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!') {
                    let arg = tokens.get(i + 2).map(|t| t.to_string()).unwrap_or_default();
                    let generated = arg.contains("\"OUT_DIR\"");
                    if !generated && (id == "include" || arg.contains(".rs\"")) {
                        scan.banned.push(format!("{id}! {arg}"));
                    }
                }
            }
            TokenTree::Punct(p) if p.as_char() == '#' => {
                let at = if matches!(tokens.get(i + 1), Some(TokenTree::Punct(q)) if q.as_char() == '!')
                {
                    i + 2
                } else {
                    i + 1
                };
                if bracket_starts_with(tokens.get(at), "path") {
                    scan.banned.push("#[path] attribute".to_string());
                }
            }
            TokenTree::Group(g) => scan_stream(g.stream(), module, scan),
            _ => {}
        }
        i += 1;
    }
}

pub fn scan_source(src: &str, module: &[String]) -> Result<Scan, String> {
    let stream = TokenStream::from_str(src).map_err(|e| e.to_string())?;
    let mut scan = Scan::default();
    scan_stream(stream, module, &mut scan);
    Ok(scan)
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

pub type Graph = BTreeMap<String, BTreeSet<String>>;

pub fn graph(root: &Path) -> Result<(Graph, Vec<String>), String> {
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();
    let mut edges = Graph::new();
    let mut problems = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(root)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        if rel == "src/main.rs" {
            continue;
        }
        let src = std::fs::read_to_string(&f).map_err(|e| format!("{rel}: {e}"))?;
        let module = module_of(&rel).ok_or_else(|| format!("{rel}: not a module file"))?;
        let scan = scan_source(&src, &module).map_err(|e| format!("{rel}: {e}"))?;
        for b in &scan.banned {
            problems.push(format!("{rel}: {b} is not allowed in src"));
        }
        let from = match unit_of(&module) {
            Some(u) if layer_of(&u).is_some() => u,
            _ if is_group_root(&module) || module.is_empty() => {
                if !scan.targets.is_empty() {
                    problems.push(format!(
                        "{rel}: a layer folder's mod.rs holds only mod lines"
                    ));
                }
                continue;
            }
            _ => {
                problems.push(format!(
                    "{rel}: module {} is in no unit of the layer table in xtask/src/layers.rs",
                    module.join("::")
                ));
                continue;
            }
        };
        for t in scan.targets {
            let Some(to) = unit_of(&t) else {
                problems.push(format!(
                    "{rel}: crate::{} names a layer folder or the crate root; import the unit itself",
                    t.join("::")
                ));
                continue;
            };
            if to == from {
                continue;
            }
            if layer_of(&to).is_none() {
                problems.push(format!(
                    "{rel}: crate::{} is in no unit of the layer table",
                    t.join("::")
                ));
                continue;
            }
            edges.entry(from.clone()).or_default().insert(to);
        }
    }
    Ok((edges, problems))
}

fn is_group_root(module: &[String]) -> bool {
    module.len() == 1 && GROUPS.contains(&module[0].as_str())
}

pub fn upward(edges: &Graph) -> Vec<String> {
    let mut out = Vec::new();
    for (from, tos) in edges {
        for to in tos {
            if let (Some(a), Some(b)) = (layer_of(from), layer_of(to))
                && b > a
            {
                out.push(format!(
                    "{from} ({a:?}) imports {to} ({b:?}), a higher layer"
                ));
            }
        }
    }
    out
}

pub fn cycles(edges: &Graph) -> Vec<String> {
    struct Tarjan<'a> {
        edges: &'a Graph,
        index: BTreeMap<&'a str, usize>,
        low: BTreeMap<&'a str, usize>,
        stack: Vec<&'a str>,
        on: BTreeSet<&'a str>,
        next: usize,
        found: Vec<Vec<String>>,
    }
    impl<'a> Tarjan<'a> {
        fn visit(&mut self, v: &'a str) {
            self.index.insert(v, self.next);
            self.low.insert(v, self.next);
            self.next += 1;
            self.stack.push(v);
            self.on.insert(v);
            for w in self.edges.get(v).into_iter().flatten() {
                let w = w.as_str();
                if !self.index.contains_key(w) {
                    self.visit(w);
                    let l = self.low[v].min(self.low[w]);
                    self.low.insert(v, l);
                } else if self.on.contains(w) {
                    let l = self.low[v].min(self.index[w]);
                    self.low.insert(v, l);
                }
            }
            if self.low[v] == self.index[v] {
                let mut part = Vec::new();
                while let Some(w) = self.stack.pop() {
                    self.on.remove(w);
                    part.push(w.to_string());
                    if w == v {
                        break;
                    }
                }
                if part.len() > 1 {
                    part.sort();
                    self.found.push(part);
                }
            }
        }
    }
    let mut t = Tarjan {
        edges,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        stack: Vec::new(),
        on: BTreeSet::new(),
        next: 0,
        found: Vec::new(),
    };
    let nodes: BTreeSet<&str> = edges
        .iter()
        .flat_map(|(a, bs)| std::iter::once(a.as_str()).chain(bs.iter().map(String::as_str)))
        .collect();
    for n in nodes {
        if !t.index.contains_key(n) {
            t.visit(n);
        }
    }
    t.found
        .into_iter()
        .map(|c| format!("these units import each other in a cycle: {}", c.join(", ")))
        .collect()
}

pub fn guard_problems(root: &Path) -> Vec<String> {
    let squash = |s: &str| s.split_whitespace().collect::<String>();
    GUARDS
        .iter()
        .filter(|(file, text, _)| {
            !std::fs::read_to_string(root.join(file))
                .is_ok_and(|src| squash(&src).contains(&squash(text)))
        })
        .map(|(file, text, why)| format!("{file} must keep `{text}`: {why}"))
        .collect()
}

pub fn problems(root: &Path) -> Result<Vec<String>, String> {
    let (edges, mut problems) = graph(root)?;
    problems.extend(upward(&edges));
    problems.extend(cycles(&edges));
    problems.extend(guard_problems(root));
    Ok(problems)
}

pub fn print_edges(root: &Path) -> Result<(), String> {
    let (edges, _) = graph(root)?;
    for (from, tos) in &edges {
        let layer = layer_of(from).map(|l| format!("{l:?}")).unwrap_or_default();
        println!(
            "{from} ({layer}) -> {}",
            tos.iter().cloned().collect::<Vec<_>>().join(" ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
