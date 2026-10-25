use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use burrow_admin_proto::schema::{Node, SemType};
use burrow_config::admin_schema::{AdminSchema, entities};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit_mut::VisitMut;

pub const FIELD_FREE: &[&str] = &[
    "crates/burrow-admin-ui/src",
    "crates/burrow-admin-logic/src",
    "crates/burrow-ui-kit/src",
    "src/admin",
    "src/admin_local",
];

const COMMON_WORDS: &[&str] = &["file", "to", "api", "server", "service", "password"];

pub fn keys() -> BTreeSet<String> {
    fn walk(node: &Node, out: &mut BTreeSet<String>) {
        match &node.sem {
            SemType::Object(o) => {
                for f in &o.fields {
                    out.insert(f.key.clone());
                    walk(&f.node, out);
                }
            }
            SemType::Map(m) => walk(&m.value, out),
            SemType::RefMap(m) => walk(&m.value, out),
            SemType::Optional(o) => walk(&o.inner, out),
            SemType::List(l) => walk(&l.item, out),
            SemType::Union(u) => u.branches.iter().for_each(|b| walk(b, out)),
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(
        &<burrow_config::ServerConfig as AdminSchema>::node(),
        &mut out,
    );
    out.extend(entities().into_iter().flat_map(|e| e.path));
    for word in COMMON_WORDS {
        out.remove(*word);
    }
    out
}

fn is_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && matches!(&a.meta, syn::Meta::List(l) if l.tokens.to_string() == "test")
    })
}

struct StripTests;

impl VisitMut for StripTests {
    fn visit_file_mut(&mut self, f: &mut syn::File) {
        f.items.retain(|i| !item_is_test(i));
        syn::visit_mut::visit_file_mut(self, f);
    }

    fn visit_item_mod_mut(&mut self, m: &mut syn::ItemMod) {
        if let Some((_, items)) = &mut m.content {
            items.retain(|i| !item_is_test(i));
        }
        syn::visit_mut::visit_item_mod_mut(self, m);
    }

    fn visit_item_impl_mut(&mut self, i: &mut syn::ItemImpl) {
        i.items.retain(|it| match it {
            syn::ImplItem::Fn(f) => !is_test(&f.attrs),
            syn::ImplItem::Const(c) => !is_test(&c.attrs),
            _ => true,
        });
        syn::visit_mut::visit_item_impl_mut(self, i);
    }
}

fn item_is_test(i: &syn::Item) -> bool {
    match i {
        syn::Item::Mod(m) => is_test(&m.attrs),
        syn::Item::Fn(f) => is_test(&f.attrs),
        syn::Item::Const(c) => is_test(&c.attrs),
        syn::Item::Static(s) => is_test(&s.attrs),
        syn::Item::Use(u) => is_test(&u.attrs),
        syn::Item::Impl(i) => is_test(&i.attrs),
        syn::Item::Struct(s) => is_test(&s.attrs),
        syn::Item::Enum(e) => is_test(&e.attrs),
        _ => false,
    }
}

fn literals(tokens: TokenStream, out: &mut Vec<(usize, String)>) {
    for t in tokens {
        match t {
            TokenTree::Group(g) => literals(g.stream(), out),
            TokenTree::Literal(l) => {
                let line = l.span().start().line;
                if let syn::Lit::Str(s) = syn::Lit::new(l) {
                    out.push((line, s.value()));
                }
            }
            _ => {}
        }
    }
}

pub fn problems_in(file: &str, src: &str, keys: &BTreeSet<String>) -> Result<Vec<String>, String> {
    let mut ast = syn::parse_file(src).map_err(|e| format!("{file}: {e}"))?;
    if is_test(&ast.attrs) {
        return Ok(Vec::new());
    }
    StripTests.visit_file_mut(&mut ast);
    let mut found = Vec::new();
    literals(ast.into_token_stream(), &mut found);
    Ok(found
        .into_iter()
        .filter(|(_, l)| keys.contains(l))
        .map(|(line, l)| {
            format!(
                "{file}:{line}: \"{l}\" names a config field; use the typed paths of burrow-config"
            )
        })
        .collect())
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

pub fn problems(root: &Path) -> Result<Vec<String>, String> {
    problems_under(root, FIELD_FREE)
}

fn problems_under(root: &Path, dirs: &[&str]) -> Result<Vec<String>, String> {
    let keys = keys();
    let mut out = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        rust_files(&root.join(dir), &mut files);
        if files.is_empty() {
            out.push(format!(
                "{dir} has no Rust files to scan for field names; update FIELD_FREE in xtask/src/fields.rs"
            ));
        }
        files.sort();
        for f in files {
            let name = f.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if name == "tests.rs" || name == "testing.rs" {
                continue;
            }
            let src = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
            let shown = f.strip_prefix(root).unwrap_or(&f).display().to_string();
            out.extend(problems_in(&shown, &src, &keys)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_come_from_the_model() {
        let k = keys();
        for key in [
            "admin_from",
            "bind",
            "services",
            "self",
            "interval_secs",
            "cert",
        ] {
            assert!(k.contains(key), "{key}");
        }
        assert!(!k.contains("online"));
    }

    #[test]
    fn nested_keys_that_are_common_words_are_not_field_names() {
        let k = keys();
        for word in COMMON_WORDS {
            assert!(!k.contains(*word), "{word}");
        }
        assert!(k.contains("cert") && k.contains("key") && k.contains("command"));
    }

    #[test]
    fn a_field_free_folder_with_no_files_is_a_problem() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("root");
        let found = problems_under(root, &["src/no-such-folder"]).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("src/no-such-folder has no Rust files"));
        for dir in FIELD_FREE {
            let mut files = Vec::new();
            rust_files(&root.join(dir), &mut files);
            assert!(!files.is_empty(), "{dir}");
        }
    }

    #[test]
    fn a_test_only_file_is_skipped() {
        let keys: BTreeSet<String> = ["bind".to_string()].into();
        let only_tests = "#![cfg(test)]\nfn a() { let x = \"bind\"; }\n";
        assert!(problems_in("x.rs", only_tests, &keys).unwrap().is_empty());
        let shipped = "#![cfg(not(test))]\nfn a() { let x = \"bind\"; }\n";
        assert_eq!(problems_in("x.rs", shipped, &keys).unwrap().len(), 1);
    }

    #[test]
    fn a_field_name_in_a_string_literal_is_found() {
        let keys: BTreeSet<String> = ["admin_from".to_string(), "bind".to_string()].into();
        let src = r##"
fn a() {
    let x = "admin_from";
    let c = '"';
    let y = r#"bind"#;
    let z = "binder";
    let w = "a \"bind\" b";
    let j = json!({"bind": 1});
    let f = format!("bind");
    let v = view! { <span data-x="admin_from"></span> };
}
#[cfg(test)]
mod tests { const K: &str = "bind"; }
#[cfg(test)]
fn helper() -> &'static str { "bind" }
#[cfg(not(test))]
fn shipped() -> &'static str { "bind" }
"##;
        let found = problems_in("x.rs", src, &keys).unwrap();
        let lines: Vec<&str> = found
            .iter()
            .map(|f| f.split(": ").next().unwrap())
            .collect();
        assert_eq!(
            lines,
            ["x.rs:3", "x.rs:5", "x.rs:8", "x.rs:9", "x.rs:10", "x.rs:17"],
            "{found:?}"
        );
    }
}
