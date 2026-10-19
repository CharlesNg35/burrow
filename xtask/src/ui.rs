use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use blake2::Digest;

use crate::tools::{self, Tools};

const WASM_TARGET: &str = "wasm32-unknown-unknown";
const CRATE: &str = "burrow-admin-ui";
const WATCHED: &[&str] = &[
    "crates/burrow-ui-kit/src",
    "crates/burrow-admin-logic/src",
    "crates/burrow-admin-ui/src",
    "crates/burrow-admin-proto/src",
    "crates/burrow-syntax/src",
];

pub fn blake2s(bytes: &[u8]) -> String {
    blake2::Blake2s256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let shown = format!("{cmd:?}");
    let status = cmd.status().map_err(|e| format!("{shown}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{shown} failed ({status})"))
    }
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
}

pub fn dist_dir(root: &Path) -> PathBuf {
    std::env::var_os("BURROW_ADMIN_UI_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("crates/burrow-admin-ui/dist"))
}

struct File {
    path: String,
    file: String,
    mime: &'static str,
}

fn mime(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("wasm") => "application/wasm",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn sri(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let d = sha2::Sha384::digest(bytes);
    format!("sha384-{}", base64(&d))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(dir.join(name), bytes).map_err(|e| format!("{name}: {e}"))
}

fn gzip(dir: &Path, name: &str) -> Result<(), String> {
    let bytes = std::fs::read(dir.join(name)).map_err(|e| e.to_string())?;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(&bytes).map_err(|e| e.to_string())?;
    let gz = enc.finish().map_err(|e| e.to_string())?;
    write(dir, &format!("{name}.gz"), &gz)
}

fn index_html(css: &str, css_sri: &str, boot: &str, boot_sri: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<meta name=\"referrer\" content=\"no-referrer\">\n<title>burrow admin</title>\n<link rel=\"icon\" href=\"data:,\">\n<link rel=\"stylesheet\" href=\"/{css}\" integrity=\"{css_sri}\">\n<script type=\"module\" src=\"/{boot}\" integrity=\"{boot_sri}\"></script>\n</head>\n<body></body>\n</html>\n"
    )
}

fn hashed(name: &str, bytes: &[u8], release: bool) -> String {
    if !release {
        return name.to_string();
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    format!("{stem}-{}.{ext}", &blake2s(bytes)[..8])
}

pub fn build(root: &Path, tools: &Tools, release: bool) -> Result<PathBuf, String> {
    let ui = root.join("crates").join(CRATE);
    let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo
        .args(["build", "--locked", "-p", CRATE, "--target", WASM_TARGET])
        .current_dir(root);
    if release {
        cargo.args(["--profile", "wasm-release"]);
    }
    run(&mut cargo)?;
    let profile = if release { "wasm-release" } else { "debug" };
    let wasm = target_dir(root)
        .join(WASM_TARGET)
        .join(profile)
        .join(format!("{}.wasm", CRATE.replace('-', "_")));
    let wasm = if wasm.exists() {
        wasm
    } else {
        target_dir(root)
            .join(WASM_TARGET)
            .join(profile)
            .join(format!("{CRATE}.wasm"))
    };
    let work = target_dir(root).join("admin-ui-work");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    run(Command::new(&tools.wasm_bindgen)
        .args([
            "--target",
            "web",
            "--no-typescript",
            "--out-name",
            "app",
            "--out-dir",
        ])
        .arg(&work)
        .arg(&wasm))?;
    let bg = work.join("app_bg.wasm");
    if release {
        let opt = work.join("app_opt.wasm");
        run(Command::new(&tools.wasm_opt)
            .args([
                "-Oz",
                "--enable-bulk-memory",
                "--enable-nontrapping-float-to-int",
                "--enable-sign-ext",
                "--enable-mutable-globals",
                "--strip-debug",
                "--strip-producers",
            ])
            .arg(&bg)
            .arg("-o")
            .arg(&opt))?;
        std::fs::rename(&opt, &bg).map_err(|e| e.to_string())?;
    }
    let css_tmp = work.join("app.css");
    let mut tw = Command::new(&tools.tailwind);
    tw.arg("-i")
        .arg(ui.join("style/entry.css"))
        .arg("-o")
        .arg(&css_tmp)
        .current_dir(&ui);
    if release {
        tw.arg("--minify");
    }
    run(&mut tw)?;

    let dist = dist_dir(root);
    if release {
        let _ = std::fs::remove_dir_all(&dist);
    } else if let Ok(rd) = std::fs::read_dir(&dist) {
        for e in rd.flatten().filter(|e| e.file_name() != "app.css") {
            let _ = std::fs::remove_file(e.path());
        }
    }
    std::fs::create_dir_all(&dist).map_err(|e| e.to_string())?;
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let wasm_bytes = read(&bg)?;
    let wasm_name = hashed("app.wasm", &wasm_bytes, release);
    write(&dist, &wasm_name, &wasm_bytes)?;
    let js_bytes = read(&work.join("app.js"))?;
    let js_name = hashed("app.js", &js_bytes, release);
    write(&dist, &js_name, &js_bytes)?;
    let css_bytes = read(&css_tmp)?;
    let css_name = hashed("app.css", &css_bytes, release);
    write(&dist, &css_name, &css_bytes)?;
    let boot = format!(
        "import init from \"./{js_name}\";\ninit({{ module_or_path: \"/{wasm_name}\" }});\n"
    );
    let boot_name = hashed("boot.js", boot.as_bytes(), release);
    write(&dist, &boot_name, boot.as_bytes())?;
    let index = index_html(
        &css_name,
        &sri(&css_bytes),
        &boot_name,
        &sri(boot.as_bytes()),
    );
    write(&dist, "index.html", index.as_bytes())?;

    let files = [
        File {
            path: "/index.html".into(),
            file: "index.html".into(),
            mime: mime("index.html"),
        },
        File {
            path: format!("/{boot_name}"),
            file: boot_name.clone(),
            mime: mime(&boot_name),
        },
        File {
            path: format!("/{js_name}"),
            file: js_name.clone(),
            mime: mime(&js_name),
        },
        File {
            path: format!("/{wasm_name}"),
            file: wasm_name.clone(),
            mime: mime(&wasm_name),
        },
        File {
            path: format!("/{css_name}"),
            file: css_name.clone(),
            mime: mime(&css_name),
        },
    ];
    let mut listed = Vec::new();
    for f in &files {
        let bytes = read(&dist.join(&f.file))?;
        let gz_size = if release {
            gzip(&dist, &f.file)?;
            std::fs::metadata(dist.join(format!("{}.gz", f.file)))
                .map(|m| m.len())
                .unwrap_or(0)
        } else {
            0
        };
        listed.push(serde_json::json!({
            "path": f.path,
            "file": f.file,
            "mime": f.mime,
            "blake2s": blake2s(&bytes),
            "size": bytes.len(),
            "gz_size": gz_size,
        }));
    }
    let version = std::fs::read_to_string(ui.join("Cargo.toml"))
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                l.strip_prefix("version = \"")
                    .map(|v| v.trim_end_matches('"').to_string())
            })
        })
        .unwrap_or_default();
    let manifest = serde_json::json!({
        "version": version,
        "release": release,
        "files": listed,
    });
    write(
        &dist,
        "manifest.json",
        serde_json::to_string_pretty(&manifest)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )?;
    let _ = std::fs::remove_dir_all(&work);
    println!("admin UI written to {}", dist.display());
    Ok(dist)
}

fn newest(root: &Path) -> SystemTime {
    fn walk(p: &Path, best: &mut SystemTime) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                walk(&path, best);
            } else if let Ok(m) = e.metadata().and_then(|m| m.modified())
                && m > *best
            {
                *best = m;
            }
        }
    }
    let mut best = SystemTime::UNIX_EPOCH;
    for w in WATCHED {
        walk(&root.join(w), &mut best);
    }
    best
}

fn watch(root: &Path, tools: &Tools) -> Result<(), String> {
    build(root, tools, false)?;
    let dist = dist_dir(root);
    let ui = root.join("crates").join(CRATE);
    let mut tw = Command::new(&tools.tailwind)
        .arg("-i")
        .arg(ui.join("style/entry.css"))
        .arg("-o")
        .arg(dist.join("app.css"))
        .arg("--watch=always")
        .current_dir(&ui)
        .spawn()
        .map_err(|e| format!("tailwindcss: {e}"))?;
    println!("watching the UI crates; refresh the browser after a rebuild (Ctrl-C stops)");
    let mut seen = newest(root);
    loop {
        std::thread::sleep(Duration::from_millis(500));
        if let Ok(Some(status)) = tw.try_wait() {
            return Err(format!("tailwindcss --watch stopped ({status})"));
        }
        let now = newest(root);
        if now > seen {
            seen = now;
            if let Err(e) = build(root, tools, false) {
                eprintln!("{e}");
            }
        }
    }
}

pub fn main(root: &Path, args: &[String]) -> Result<(), String> {
    let tools = tools::run(root)?;
    if args.iter().any(|a| a == "--watch") {
        return watch(root, &tools);
    }
    let release = args.iter().any(|a| a == "--release");
    build(root, &tools, release).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subresource_integrity_uses_standard_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert!(sri(b"x").starts_with("sha384-"));
    }

    #[test]
    fn the_page_has_no_inline_script_or_style() {
        let html = index_html("app-1.css", "sha384-a", "boot-2.js", "sha384-b");
        assert!(html.contains(
            "<script type=\"module\" src=\"/boot-2.js\" integrity=\"sha384-b\"></script>"
        ));
        assert!(!html.contains("<style"));
        assert_eq!(html.matches("<script").count(), 1);
        assert_eq!(hashed("app.js", b"x", false), "app.js");
        assert!(hashed("app.js", b"x", true).starts_with("app-"));
    }
}
