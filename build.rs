use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rustc-env=BURROW_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
    println!("cargo::rustc-check-cfg=cfg(admin_ui_placeholder)");
    if std::env::var_os("CARGO_FEATURE_ADMIN_UI").is_some() {
        assets();
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn blake2s(bytes: &[u8]) -> String {
    use blake2::Digest;
    hex(&blake2::Blake2s256::digest(bytes))
}

fn dist_dir() -> Option<PathBuf> {
    println!("cargo:rerun-if-env-changed=BURROW_ADMIN_UI_DIST");
    println!("cargo:rerun-if-env-changed=BURROW_ADMIN_UI_REQUIRE");
    let dir = match std::env::var_os("BURROW_ADMIN_UI_DIST") {
        Some(d) => PathBuf::from(d),
        None => Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
            .join("crates/burrow-admin-ui/dist"),
    };
    println!("cargo:rerun-if-changed={}", dir.display());
    println!(
        "cargo:rerun-if-changed={}",
        dir.join("manifest.json").display()
    );
    dir.join("manifest.json").exists().then_some(dir)
}

struct Entry {
    path: String,
    mime: String,
    file: PathBuf,
    gzipped: bool,
    hash: String,
    immutable: bool,
}

fn assets() {
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let required = std::env::var_os("BURROW_ADMIN_UI_REQUIRE").is_some_and(|v| v == "1");
    let (entries, manifest_hash, version, placeholder) = match dist_dir() {
        Some(dir) => {
            let manifest = std::fs::read_to_string(dir.join("manifest.json"))
                .expect("dist/manifest.json is readable");
            let parsed: serde_json::Value =
                serde_json::from_str(&manifest).expect("dist/manifest.json is JSON");
            let mut entries = Vec::new();
            for obj in parsed["files"].as_array().into_iter().flatten() {
                let s = |k: &str| obj[k].as_str().map(str::to_string);
                let (Some(path), Some(file), Some(mime), Some(hash)) =
                    (s("path"), s("file"), s("mime"), s("blake2s"))
                else {
                    panic!(
                        "dist/manifest.json has an entry without path, file, mime or blake2s: {obj}"
                    );
                };
                let raw = dir.join(&file);
                let bytes = std::fs::read(&raw).unwrap_or_else(|e| {
                    panic!(
                        "{} is listed in dist/manifest.json but cannot be read: {e}",
                        raw.display()
                    )
                });
                assert_eq!(
                    blake2s(&bytes),
                    hash,
                    "{} does not match its BLAKE2s in dist/manifest.json; rebuild with cargo xtask ui",
                    raw.display()
                );
                let gz = dir.join(format!("{file}.gz"));
                let (file, gzipped) = if gz.exists() {
                    (gz, true)
                } else {
                    (raw, false)
                };
                println!("cargo:rerun-if-changed={}", file.display());
                entries.push(Entry {
                    immutable: path != "/index.html",
                    path,
                    mime,
                    file,
                    gzipped,
                    hash: hash[..16].to_string(),
                });
            }
            let version = parsed["version"].as_str().unwrap_or("unknown").to_string();
            (
                entries,
                blake2s(manifest.as_bytes())[..16].to_string(),
                version,
                false,
            )
        }
        None => {
            assert!(
                !required,
                "BURROW_ADMIN_UI_REQUIRE=1 but no admin UI was built; run cargo xtask ui first, or set BURROW_ADMIN_UI_DIST"
            );
            let page = out.join("placeholder.html");
            std::fs::write(&page, PLACEHOLDER).expect("write the placeholder page");
            (
                vec![Entry {
                    path: "/index.html".into(),
                    mime: "text/html; charset=utf-8".into(),
                    file: page,
                    gzipped: false,
                    hash: blake2s(PLACEHOLDER.as_bytes())[..16].to_string(),
                    immutable: false,
                }],
                "placeholder".to_string(),
                "none".to_string(),
                true,
            )
        }
    };
    if placeholder {
        println!("cargo:rustc-cfg=admin_ui_placeholder");
    }
    let mut code = String::from("pub static ASSETS: &[Asset] = &[\n");
    for e in &entries {
        code.push_str(&format!(
            "    Asset {{ path: {:?}, mime: {:?}, body: include_bytes!({:?}), gzipped: {}, etag: {:?}, immutable: {} }},\n",
            e.path,
            e.mime,
            e.file.display().to_string(),
            e.gzipped,
            e.hash,
            e.immutable
        ));
    }
    code.push_str("];\n");
    code.push_str(&format!(
        "pub const UI_MANIFEST_HASH: &str = {manifest_hash:?};\n"
    ));
    code.push_str(&format!("pub const UI_VERSION: &str = {version:?};\n"));
    std::fs::write(out.join("admin_assets.rs"), code).expect("write admin_assets.rs");
}

const PLACEHOLDER: &str = "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>burrow admin</title>\n</head>\n<body>\n<h1>burrow admin</h1>\n<p>The admin UI was not built into this binary. Release binaries include it; to build it yourself, run <code>cargo xtask ui</code> and build burrow again.</p>\n</body>\n</html>\n";
