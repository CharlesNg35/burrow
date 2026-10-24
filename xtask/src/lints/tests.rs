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
fn the_tree_keeps_its_lint_rules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("root");
    let found = problems(root).unwrap();
    assert!(found.is_empty(), "{found:#?}");
}
