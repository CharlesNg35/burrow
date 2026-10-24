use super::*;

#[test]
fn too_many_lines_and_its_groups_cannot_be_lowered_in_code() {
    for c in [
        "#[allow(clippy::too_many_lines)]",
        "#[expect(clippy::too_many_lines)]",
        "#[warn(clippy::too_many_lines)]",
        "#![allow(clippy::all, clippy::too_many_lines)]",
        "#![allow(clippy::pedantic)]",
        "#[allow(clippy::all)]",
        "#![allow(warnings)]",
        "#[expect(warnings)]",
        "#[cfg_attr(test, allow(clippy::too_many_lines))]",
        "#[allow( clippy :: too_many_lines )]",
        "#[allow(\n    clippy::needless_borrow,\n    clippy::too_many_lines,\n)]",
    ] {
        let src = format!("{c}\nfn a() {{}}\n");
        assert_eq!(silenced_problems("src/a.rs", &src).len(), 1, "{c}");
    }
    let multi =
        "#[allow(\n    clippy::needless_borrow,\n    clippy::too_many_lines,\n)]\nfn a() {}\n";
    let found = silenced_problems("src/a.rs", multi);
    assert!(
        found[0].starts_with("src/a.rs:1: allow(clippy::needless_borrow,clippy::too_many_lines,)"),
        "{found:?}"
    );
}

#[test]
fn other_lints_strings_and_calls_are_not_silencing() {
    for c in [
        "#[allow(clippy::too_many_arguments)]",
        "#![allow(clippy::disallowed_macros)]",
        "#[warn(warnings)]",
        "#[deny(clippy::all)]",
        "let s = \"#[allow(clippy::too_many_lines)]\";",
        "if let Some(more) = REFUSED.allow() {}",
        "fn allow(x: u8) {}",
    ] {
        assert!(
            silenced_problems("src/a.rs", &format!("{c}\n")).is_empty(),
            "{c}"
        );
    }
}

#[test]
fn the_crate_has_no_other_name() {
    for c in [
        "use crate as root;",
        "use crate::{self as root, wire};",
        "extern crate self as burrow;",
        "extern crate alloc;",
    ] {
        assert_eq!(
            alias_problems("src/lib.rs", &format!("{c}\n")).len(),
            1,
            "{c}"
        );
    }
    for c in [
        "use crate::wire::{self, noise};",
        "use crate::wire::noise as n;",
        "let s = \"extern crate self as burrow;\";",
    ] {
        assert!(
            alias_problems("src/lib.rs", &format!("{c}\n")).is_empty(),
            "{c}"
        );
    }
}

#[test]
fn a_member_takes_the_workspace_lints_as_they_are() {
    let ok = "[package]\nname = \"a\"\n\n[lints]\nworkspace = true\n\n[dependencies]\n";
    assert!(member_lint_problems("a/Cargo.toml", ok).is_empty());
    for bad in [
        "[package]\nname = \"a\"\n",
        "[lints]\nworkspace = true\nclippy.too_many_lines = \"allow\"\n",
        "[lints]\nworkspace = true\n\n[lints.clippy]\ntoo_many_lines = \"allow\"\n",
        "[lints.clippy]\ntoo_many_lines = \"allow\"\n",
        "[lints]\nworkspace = false\n",
    ] {
        assert_eq!(member_lint_problems("a/Cargo.toml", bad).len(), 1, "{bad}");
    }
}

#[test]
fn the_root_keeps_the_threshold_and_the_deny() {
    let cargo =
        "[workspace.lints.clippy]\ntoo_many_lines = \"deny\"\n\n[lints]\nworkspace = true\n";
    let clippy = "disallowed-macros = []\ntoo-many-lines-threshold = 150\n";
    assert!(root_problems(cargo, clippy).is_empty());
    assert_eq!(
        root_problems(cargo, "too-many-lines-threshold = 200\n").len(),
        1
    );
    let warn = "[workspace.lints.clippy]\ntoo_many_lines = \"warn\"\n";
    assert_eq!(root_problems(warn, clippy).len(), 1);
    assert_eq!(root_problems("", "").len(), 2);
}

#[test]
fn members_come_from_the_workspace_list() {
    let cargo = "[workspace]\nmembers = [\n    \".\",\n    \"crates/a\",\n    \"xtask\",\n]\n";
    assert_eq!(members(cargo), [".", "crates/a", "xtask"]);
}

#[test]
fn flags_that_loosen_clippy_are_found() {
    for bad in [
        "rustflags = [\"-A\", \"clippy::all\"]\nRUSTFLAGS: -A clippy::too_many_lines",
        "run: cargo clippy -- -D warnings -Aclippy::too_many_lines",
        "run: cargo clippy -- --allow clippy::all",
        "run: cargo clippy -- -W clippy::too_many_lines",
    ] {
        assert!(!flag_problems("ci.yml", bad).is_empty(), "{bad}");
    }
    let ok = "run: cargo clippy --locked --workspace --all-targets -- -D warnings\n";
    assert!(flag_problems("ci.yml", ok).is_empty());
}

#[test]
fn the_tree_keeps_its_lint_rules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("root");
    let found = problems(root).unwrap();
    assert!(found.is_empty(), "{found:#?}");
}
