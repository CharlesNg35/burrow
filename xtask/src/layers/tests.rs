use super::*;

fn m(path: &str) -> Vec<String> {
    path.split("::").map(str::to_string).collect()
}

fn targets(src: &str, module: &str) -> Vec<String> {
    scan_source(src, &m(module))
        .unwrap()
        .targets
        .into_iter()
        .map(|t| t.join("::"))
        .collect()
}

#[test]
fn files_map_to_modules_and_units() {
    assert_eq!(module_of("src/server/mod.rs").unwrap(), m("server"));
    assert_eq!(module_of("src/base/fs.rs").unwrap(), m("base::fs"));
    assert_eq!(
        module_of("src/wire/transport/tcp.rs").unwrap(),
        m("wire::transport::tcp")
    );
    assert!(module_of("src/lib.rs").unwrap().is_empty());
    assert_eq!(
        unit_of(&m("wire::transport::tcp")).unwrap(),
        "wire::transport"
    );
    assert_eq!(unit_of(&m("server::visit")).unwrap(), "server");
    assert_eq!(unit_of(&m("say_errln")).unwrap(), "base::failure");
    assert_eq!(unit_of(&m("base")), None);
    for (unit, _) in UNITS {
        assert_eq!(unit_of(&m(unit)).as_deref(), Some(*unit));
    }
}

#[test]
fn paths_are_found_in_uses_groups_macros_and_inline_modules() {
    let src = r#"
use crate::wire::transport::{self, Channel, tcp::{tune, SocketTuning}};
use super::sibling::Thing;
fn f() {
    let s = "crate::admin::Hub";
    assert_eq!(crate::config::env::referenced_env(""), vec![]);
    crate::say_errln!("x");
    let me = self::local::X;
}
macro_rules! m { () => { $crate::base::failure::to_stderr("") }; }
mod tests {
    use super::super::other::Y;
}
"#;
    let found = targets(src, "server::visit");
    for want in [
        "wire::transport",
        "wire::transport::Channel",
        "wire::transport::tcp::tune",
        "wire::transport::tcp::SocketTuning",
        "server::sibling::Thing",
        "config::env::referenced_env",
        "say_errln",
        "server::visit::local::X",
        "base::failure::to_stderr",
        "server::other::Y",
    ] {
        assert!(found.iter().any(|f| f == want), "{want} in {found:?}");
    }
    assert!(!found.iter().any(|f| f.starts_with("admin")), "{found:?}");
}

#[test]
fn a_bare_layer_folder_or_crate_root_import_is_found_and_has_no_unit() {
    for src in [
        "use crate::wire;\n",
        "use crate::wire::*;\n",
        "use crate::wire::{self};\n",
        "use crate::base;\n",
        "use crate::*;\n",
    ] {
        let found = scan_source(src, &m("server")).unwrap().targets;
        assert_eq!(found.len(), 1, "{src}: {found:?}");
        assert_eq!(unit_of(&found[0]), None, "{src}");
    }
    assert!(targets("pub struct P(pub(in crate::wire) ());\n", "wire::noise").is_empty());
}

#[test]
fn burrow_paths_are_scanned_like_crate_paths() {
    let src = "use burrow::admin::Hub;\nuse ::burrow::server::Shared;\nfn f() { let t = \"burrow::admin\"; let x = other::burrow::thing(); }\n";
    let found = targets(src, "wire::noise");
    assert!(found.iter().any(|f| f == "admin::Hub"), "{found:?}");
    assert!(found.iter().any(|f| f == "server::Shared"), "{found:?}");
    assert_eq!(found.len(), 2, "{found:?}");
}

#[test]
fn super_crosses_into_a_sibling_unit_of_a_layer_folder() {
    let found = targets("use super::failure::Failure;\n", "base::fs");
    assert_eq!(found, ["base::failure::Failure"]);
    assert_eq!(unit_of(&m(&found[0])).unwrap(), "base::failure");
}

#[test]
fn path_attributes_and_source_includes_are_banned() {
    let src = "#[path = \"x.rs\"]\nmod x;\ninclude!(\"y.rs\");\ninclude!(concat!(env!(\"OUT_DIR\"), \"/assets.rs\"));\nconst A: &str = include_str!(\"z.rs\");\nconst B: &str = include_str!(\"page.html\");\n";
    let scan = scan_source(src, &m("server")).unwrap();
    assert_eq!(scan.banned.len(), 3, "{:?}", scan.banned);
}

#[test]
fn an_upward_import_and_a_cycle_are_problems() {
    let mut g = Graph::new();
    g.entry("wire::noise".into())
        .or_default()
        .insert("config".into());
    g.entry("config".into())
        .or_default()
        .insert("wire::noise".into());
    g.entry("server".into())
        .or_default()
        .insert("client".into());
    let up = upward(&g);
    assert_eq!(up.len(), 1, "{up:?}");
    assert!(up[0].starts_with("wire::noise (Wire) imports config (Config)"));
    let c = cycles(&g);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(c[0].ends_with("config, wire::noise"));
    let mut line = Graph::new();
    line.entry("visitor".into())
        .or_default()
        .insert("server".into());
    line.entry("server".into())
        .or_default()
        .insert("client".into());
    assert!(cycles(&line).is_empty());
    assert!(upward(&line).is_empty());
}

#[test]
fn the_tree_follows_the_layers_and_keeps_its_guards() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("root");
    let found = problems(root).unwrap();
    assert!(found.is_empty(), "{found:#?}");
    let (edges, _) = graph(root).unwrap();
    assert!(edges.len() > 20);
}

#[test]
fn a_missing_guard_is_a_problem() {
    let dir = std::env::temp_dir().join(format!("burrow-guard-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src/wire")).unwrap();
    std::fs::write(
        dir.join("src/wire/noise.rs"),
        "pub struct InMemory(pub(crate) ());\n",
    )
    .unwrap();
    let found = guard_problems(&dir);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
}
