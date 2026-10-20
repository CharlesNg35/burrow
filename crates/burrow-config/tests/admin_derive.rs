use std::path::{Path, PathBuf};
use std::process::Command;

const FAIL: &[(&str, &[&str])] = &[
    (
        "missing_field_attr",
        &["field `max_visitors` of `Limits` needs `#[admin]`, or `#[admin(skip = \"why\")]`"],
    ),
    (
        "missing_variant_attr",
        &["variant `Random` of `Mode` needs #[admin]"],
    ),
    (
        "redundant_label",
        &[
            "redundant label: \"Max visitors\" is already derived from the name",
            "redundant label: \"v1\" is already derived from the name",
        ],
    ),
    (
        "redundant_ty",
        &[
            "redundant ty on `max_visitors`: its Rust type already gives the semantic type `integer`",
        ],
    ),
    (
        "redundant_duration",
        &["redundant ty: the `_secs` or `_ms` name already makes this a duration"],
    ),
    (
        "skip_without_reason",
        &["`skip` needs a reason: #[admin(skip = \"why\")]"],
    ),
    (
        "wrong_carrier",
        &["`u32` cannot carry the semantic type `Rate`"],
    ),
    (
        "relation_missing_field",
        &["`clientz` is not a field of `Service`"],
    ),
    (
        "reduce_only_conflicts",
        &[
            "`reduce_only` and `locked` contradict",
            "`reduce_only` and `readonly` contradict",
            "`reduce_only` is a flag: #[admin(reduce_only)]",
            "`reduce_only` and `skip` contradict",
        ],
    ),
    (
        "path_wrong_parent",
        &["mismatched types", "expected `Field<Limits, _>`"],
    ),
    (
        "remote_missing_variant",
        &["non-exhaustive patterns: `&Level::Owner` not covered"],
    ),
    (
        "unknown_key",
        &[
            "unknown key `lable` in #[admin(...)]",
            "unknown semantic type `colour`",
        ],
    ),
];

fn scratch() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("admin-derive");
    let src = dir.join("src/bin");
    std::fs::create_dir_all(&src).unwrap();
    let mut bins = Vec::new();
    for group in ["fail", "pass"] {
        for entry in std::fs::read_dir(root.join("tests/admin_derive").join(group)).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            std::fs::copy(&path, src.join(format!("{name}.rs"))).unwrap();
            bins.push(name);
        }
    }
    let manifest = format!(
        "[package]\nname = \"admin-derive-cases\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n[dependencies]\nburrow-config = {{ path = {:?} }}\nserde = {{ version = \"1\", features = [\"derive\"] }}\n",
        root.display().to_string()
    );
    std::fs::write(dir.join("Cargo.toml"), manifest).unwrap();
    let lock = root.join("../../Cargo.lock");
    if lock.exists() && !dir.join("Cargo.lock").exists() {
        std::fs::copy(lock, dir.join("Cargo.lock")).unwrap();
    }
    dir
}

fn cargo(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(args)
        .arg("--quiet")
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .env("CARGO_TERM_COLOR", "never")
        .current_dir(dir)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn admin_schema_derive_refuses_undescribed_fields() {
    let dir = scratch();
    let (ok, err) = cargo(&dir, &["run", "--bin", "all_attributes"]);
    assert!(ok, "the pass case does not build or run:\n{err}");
    for (case, wanted) in FAIL {
        let (ok, err) = cargo(&dir, &["check", "--bin", case]);
        assert!(!ok, "{case} compiled but must not");
        for w in *wanted {
            assert!(err.contains(w), "{case}: expected {w:?} in:\n{err}");
        }
    }
}
