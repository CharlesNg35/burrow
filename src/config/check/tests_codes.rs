use std::path::Path;

fn sources(dir: &Path, out: &mut String) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            sources(&path, out);
        } else if name.ends_with(".rs") && !name.starts_with("tests") {
            out.push_str(&std::fs::read_to_string(&path).unwrap());
        }
    }
}

#[test]
fn every_code_the_admin_ui_reads_is_emitted_by_validate() {
    let mut text = String::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src/config/check"),
        &mut text,
    );
    let missing: Vec<&str> = burrow_admin_proto::codes::READ_BY_THE_UI
        .iter()
        .copied()
        .filter(|code| {
            let name = code.to_ascii_uppercase().replace('-', "_");
            !text.contains(&name) || text.contains(&format!("\"{code}\""))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "each code is emitted through burrow_admin_proto::codes, never spelled: {missing:?}"
    );
}

#[test]
fn every_code_the_cli_reads_is_emitted_through_its_const() {
    let mut text = String::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src/config"),
        &mut text,
    );
    let missing: Vec<&str> = burrow_admin_proto::codes::READ_BY_THE_CLI
        .iter()
        .copied()
        .filter(|code| {
            let name = code.to_ascii_uppercase().replace('-', "_");
            !text.contains(&format!("codes::{name}")) || text.contains(&format!("\"{code}\""))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "each code is emitted through burrow_admin_proto::codes, never spelled: {missing:?}"
    );
}
