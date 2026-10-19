use std::path::{Path, PathBuf};
use std::process::Command;

use syn::visit::Visit;

struct Rule {
    dir: &'static str,
    forbid: &'static [&'static str],
    only: Option<&'static [&'static str]>,
    why: &'static str,
}

const ROOT_RULES: &[Rule] = &[
    Rule {
        dir: "src/admin_local",
        forbid: &[],
        only: Some(&[
            "crate::admin_local",
            "crate::dialer",
            "crate::config::settings::VisitorSettings",
            "crate::instance",
            "crate::failure",
            "burrow_admin_proto::frame",
        ]),
        why: "the local bridge knows HTTP, WebSocket, tokens and frames only",
    },
    Rule {
        dir: "src/admin/transport",
        forbid: &["crate::config", "crate::admin::adapters"],
        only: None,
        why: "admin transport knows sessions, frames and limits, not config text",
    },
    Rule {
        dir: "src/admin/core",
        forbid: &[
            "crate::handshake",
            "crate::record",
            "tokio::net",
            "crate::server",
            "crate::reload",
            "crate::admin::transport",
            "crate::admin::adapters",
            "crate::admin_local",
        ],
        only: None,
        why: "admin core reaches sockets, files and the server only through its ports",
    },
    Rule {
        dir: "src/config",
        forbid: &["crate::admin", "crate::admin_local"],
        only: None,
        why: "config must not know admin sessions or the UI",
    },
];

const UI_RULES: &[Rule] = &[
    Rule {
        dir: "crates/burrow-admin-ui/src/domain",
        forbid: &[
            "crate::api",
            "crate::screens",
            "crate::store",
            "burrow_admin_proto::msg",
            "burrow_admin_proto::frame",
        ],
        only: None,
        why: "domain components know semantic types and value grammar only",
    },
    Rule {
        dir: "crates/burrow-admin-ui/src/form",
        forbid: &["crate::api", "crate::screens", "burrow_admin_proto::frame"],
        only: None,
        why: "the form layer maps schema nodes to components, it sends no requests",
    },
    Rule {
        dir: "crates/burrow-admin-ui/src/screens",
        forbid: &["burrow_admin_proto::frame"],
        only: None,
        why: "screens do not know wire framing",
    },
    Rule {
        dir: "crates/burrow-admin-ui/src/api",
        forbid: &["crate::domain", "crate::form", "crate::screens"],
        only: None,
        why: "the API layer knows envelopes, not components",
    },
];

const CRATE_DEPS: &[(&str, &[&str], &[&str])] = &[
    ("burrow-syntax", &[], &[]),
    ("burrow-admin-proto", &["serde", "serde_json"], &[]),
    ("burrow-admin-derive", &["proc-macro2", "quote", "syn"], &[]),
    (
        "burrow-admin-logic",
        &["burrow-admin-proto", "burrow-syntax"],
        &[],
    ),
    (
        "burrow-ui-kit",
        &["leptos", "wasm-bindgen", "web-sys", "js-sys"],
        &["burrow-admin-proto", "burrow-syntax", "burrow-admin-logic"],
    ),
    (
        "burrow-admin-ui",
        &[
            "leptos",
            "wasm-bindgen",
            "wasm-bindgen-futures",
            "web-sys",
            "js-sys",
            "burrow-admin-proto",
            "burrow-admin-logic",
            "burrow-ui-kit",
            "burrow-syntax",
            "console_error_panic_hook",
        ],
        &["serde_json"],
    ),
];

const ROOT_NEVER: &[&str] = &["burrow-ui-kit", "burrow-admin-logic", "burrow-admin-ui"];

pub fn run(root: &Path) -> Result<(), String> {
    let mut problems = Vec::new();
    problems.extend(check_deps(root)?);
    for rule in ROOT_RULES.iter().chain(UI_RULES) {
        for file in rust_files(&root.join(rule.dir)) {
            let rel = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
            let src =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", rel.display()))?;
            problems.extend(check_source(rule, &rel, &src)?);
        }
    }
    for file in rust_files(&root.join("src")) {
        let rel = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
        if rel.starts_with("src/admin") {
            continue;
        }
        let src = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", rel.display()))?;
        let rule = Rule {
            dir: "src",
            forbid: &[
                "crate::admin::core",
                "crate::admin::transport",
                "crate::admin::adapters",
            ],
            only: None,
            why: "the rest of burrow enters the admin feature through crate::admin only",
        };
        problems.extend(check_source(&rule, &rel, &src)?);
    }
    problems.extend(check_ui_words(root));
    if problems.is_empty() {
        println!("arch-check: ok");
        Ok(())
    } else {
        for p in &problems {
            eprintln!("{p}");
        }
        Err(format!("arch-check: {} violations", problems.len()))
    }
}

fn check_deps(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let mut problems = Vec::new();
    let empty = Vec::new();
    for pkg in meta["packages"].as_array().unwrap_or(&empty) {
        let name = pkg["name"].as_str().unwrap_or_default();
        let deps: Vec<&str> = pkg["dependencies"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter(|d| d["kind"].is_null())
            .filter_map(|d| d["name"].as_str())
            .collect();
        problems.extend(dep_problems(name, &deps));
    }
    Ok(problems)
}

fn dep_problems(name: &str, deps: &[&str]) -> Vec<String> {
    let mut problems = Vec::new();
    if name == "burrow" {
        for d in deps.iter().filter(|d| ROOT_NEVER.contains(d)) {
            problems.push(format!(
                "crate burrow must not depend on {d} (UI crates are wasm only)"
            ));
        }
    }
    if let Some((_, allowed, never)) = CRATE_DEPS.iter().find(|(n, _, _)| *n == name) {
        for d in deps {
            if never.contains(d) {
                problems.push(format!("crate {name} must not depend on {d}"));
            } else if !allowed.contains(d) {
                problems.push(format!(
                    "crate {name} may depend only on [{}], not on {d}",
                    allowed.join(", ")
                ));
            }
        }
    }
    problems
}

fn check_ui_words(root: &Path) -> Vec<String> {
    let dir = root.join("crates/burrow-admin-ui/src");
    let mut problems = Vec::new();
    for file in rust_files(&dir) {
        let rel = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        problems.extend(ui_words(&rel, &src));
    }
    problems
}

fn ui_words(rel: &Path, src: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let json_ok = rel.starts_with("crates/burrow-admin-ui/src/store")
        || rel.starts_with("crates/burrow-admin-ui/src/api");
    let Ok(file) = syn::parse_file(src) else {
        return vec![format!("{}: does not parse", rel.display())];
    };
    let mut idents = Idents(Vec::new());
    idents.visit_file(&file);
    for id in idents.0 {
        let lower = id.to_ascii_lowercase();
        if lower.contains("yaml") {
            problems.push(format!(
                "{}: identifier `{id}`: the UI never handles YAML",
                rel.display()
            ));
        }
        if lower.contains("json") && !json_ok {
            problems.push(format!(
                "{}: identifier `{id}`: JSON handling belongs in store/ or api/",
                rel.display()
            ));
        }
        if id == "inner_html" || id == "set_inner_html" {
            problems.push(format!(
                "{}: `{id}`: server text must never become HTML",
                rel.display()
            ));
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

struct Idents(Vec<String>);

impl<'ast> Visit<'ast> for Idents {
    fn visit_ident(&mut self, i: &'ast syn::Ident) {
        self.0.push(i.to_string());
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        for tt in m.tokens.clone() {
            collect_tokens(tt, &mut self.0);
        }
        syn::visit::visit_macro(self, m);
    }
}

fn collect_tokens(tt: proc_macro2::TokenTree, out: &mut Vec<String>) {
    match tt {
        proc_macro2::TokenTree::Ident(i) => out.push(i.to_string()),
        proc_macro2::TokenTree::Group(g) => {
            g.stream().into_iter().for_each(|t| collect_tokens(t, out))
        }
        _ => {}
    }
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            out.extend(rust_files(&p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    out
}

fn module_of(rel: &Path) -> Vec<String> {
    let mut parts: Vec<String> = rel
        .iter()
        .map(|c| c.to_string_lossy().into_owned())
        .collect();
    let start = parts.iter().position(|p| p == "src").map_or(0, |i| i + 1);
    parts.drain(..start);
    if let Some(last) = parts.last_mut() {
        *last = last.trim_end_matches(".rs").to_string();
    }
    if matches!(
        parts.last().map(String::as_str),
        Some("mod" | "lib" | "main")
    ) {
        parts.pop();
    }
    parts
}

struct Paths {
    module: Vec<String>,
    found: Vec<(String, usize)>,
}

impl Paths {
    fn resolve(&self, segs: &[String]) -> String {
        let mut base: Vec<String> = Vec::new();
        let mut rest = segs;
        match segs.first().map(String::as_str) {
            Some("crate") => {
                base.push("crate".into());
                rest = &segs[1..];
            }
            Some("super") | Some("self") => {
                base.push("crate".into());
                base.extend(self.module.iter().cloned());
                while let Some(first) = rest.first() {
                    match first.as_str() {
                        "super" => {
                            if base.len() > 1 {
                                base.pop();
                            }
                        }
                        "self" => {}
                        _ => break,
                    }
                    rest = &rest[1..];
                }
            }
            _ => {}
        }
        base.extend(rest.iter().cloned());
        base.join("::")
    }

    fn add(&mut self, segs: Vec<String>, line: usize) {
        if segs.len() > 1 {
            let full = self.resolve(&segs);
            self.found.push((full, line));
        }
    }
}

fn use_paths(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            use_paths(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let mut v = prefix.clone();
            if n.ident != "self" {
                v.push(n.ident.to_string());
            }
            out.push(v);
        }
        syn::UseTree::Rename(r) => {
            let mut v = prefix.clone();
            v.push(r.ident.to_string());
            out.push(v);
        }
        syn::UseTree::Glob(_) => out.push(prefix.clone()),
        syn::UseTree::Group(g) => g.items.iter().for_each(|t| use_paths(t, prefix, out)),
    }
}

impl<'ast> Visit<'ast> for Paths {
    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        let mut out = Vec::new();
        use_paths(&u.tree, &mut Vec::new(), &mut out);
        let line = u.use_token.span.start().line;
        for segs in out {
            self.add(segs, line);
        }
    }

    fn visit_path(&mut self, p: &'ast syn::Path) {
        let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        let line = p
            .segments
            .first()
            .map_or(0, |s| s.ident.span().start().line);
        self.add(segs, line);
        syn::visit::visit_path(self, p);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        let mut idents = Vec::new();
        let mut last_colon = false;
        let mut run: Vec<String> = Vec::new();
        for tt in m.tokens.clone() {
            match tt {
                proc_macro2::TokenTree::Ident(i) => {
                    if !last_colon && !run.is_empty() {
                        idents.push(std::mem::take(&mut run));
                    }
                    run.push(i.to_string());
                    last_colon = false;
                }
                proc_macro2::TokenTree::Punct(p) if p.as_char() == ':' => last_colon = true,
                _ => {
                    if !run.is_empty() {
                        idents.push(std::mem::take(&mut run));
                    }
                    last_colon = false;
                }
            }
        }
        if !run.is_empty() {
            idents.push(run);
        }
        let line = m
            .path
            .segments
            .first()
            .map_or(0, |s| s.ident.span().start().line);
        for segs in idents {
            self.add(segs, line);
        }
        syn::visit::visit_macro(self, m);
    }
}

fn check_source(rule: &Rule, rel: &Path, src: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(src).map_err(|e| format!("{}: {e}", rel.display()))?;
    let mut paths = Paths {
        module: module_of(rel),
        found: Vec::new(),
    };
    paths.visit_file(&file);
    let within = |p: &str, prefix: &str| p == prefix || p.starts_with(&format!("{prefix}::"));
    let mut problems = Vec::new();
    for (p, line) in paths.found {
        let bad_forbid = rule.forbid.iter().find(|f| within(&p, f));
        let bad_only = rule.only.and_then(|only| {
            let governed = p.starts_with("crate::") || p.starts_with("burrow_admin_proto::");
            (governed && !only.iter().any(|o| within(&p, o) || within(o, &p))).then_some(())
        });
        if let Some(f) = bad_forbid {
            problems.push(format!(
                "{}:{line}: uses {p}, but {} may not use {f}: {}",
                rel.display(),
                rule.dir,
                rule.why
            ));
        } else if bad_only.is_some() {
            problems.push(format!(
                "{}:{line}: uses {p}, outside what {} may use: {}",
                rel.display(),
                rule.dir,
                rule.why
            ));
        }
    }
    problems.sort();
    problems.dedup();
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(dir: &str) -> &'static Rule {
        ROOT_RULES
            .iter()
            .chain(UI_RULES)
            .find(|r| r.dir == dir)
            .expect("rule")
    }

    #[test]
    fn a_bad_import_is_found() {
        let src = "use crate::config::{model::ServerConfig, settings::VisitorSettings};\nfn f() { let _ = super::super::super::handshake::REFUSED; }\n";
        let found = check_source(
            rule("src/admin/core"),
            Path::new("src/admin/core/plan.rs"),
            src,
        )
        .unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("crate::admin::handshake") || found[0].contains("crate::handshake")
        );
        let t = check_source(
            rule("src/admin/transport"),
            Path::new("src/admin/transport/session.rs"),
            src,
        )
        .unwrap();
        assert!(
            t.iter()
                .any(|p| p.contains("crate::config::model::ServerConfig")),
            "{t:?}"
        );
        let local = "use crate::dialer::Dialer;\nuse crate::config::settings::VisitorSettings;\nuse burrow_admin_proto::frame::chunks;\nuse burrow_admin_proto::msg::Request;\nfn f() { crate::config::load::load_file(); }\n";
        let l = check_source(
            rule("src/admin_local"),
            Path::new("src/admin_local/bridge.rs"),
            local,
        )
        .unwrap();
        assert_eq!(l.len(), 2, "{l:?}");
        let fine = "use std::sync::Arc;\nuse tokio::sync::Mutex;\nuse super::ports::ConfigStore;\n";
        assert!(
            check_source(
                rule("src/admin/core"),
                Path::new("src/admin/core/plan.rs"),
                fine
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn ui_layers_are_checked() {
        let src = "use crate::api::Client;\nfn yaml_text() {}\n";
        let d = check_source(
            rule("crates/burrow-admin-ui/src/domain"),
            Path::new("crates/burrow-admin-ui/src/domain/ip.rs"),
            src,
        )
        .unwrap();
        assert_eq!(d.len(), 1);
        let w = ui_words(
            Path::new("crates/burrow-admin-ui/src/domain/ip.rs"),
            "fn f(x: u8) { let json = x; view! { <div inner_html=json/> } }",
        );
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(
            ui_words(
                Path::new("crates/burrow-admin-ui/src/store/mod.rs"),
                "fn to_json() {}"
            )
            .is_empty()
        );
    }

    #[test]
    fn dependency_rules() {
        assert!(dep_problems("burrow-admin-proto", &["serde", "serde_json"]).is_empty());
        assert_eq!(
            dep_problems("burrow-admin-proto", &["serde", "tokio"]).len(),
            1
        );
        assert_eq!(
            dep_problems("burrow-ui-kit", &["leptos", "burrow-admin-proto"]).len(),
            1
        );
        assert_eq!(dep_problems("burrow", &["burrow-admin-ui"]).len(), 1);
    }
}
