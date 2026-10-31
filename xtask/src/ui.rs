use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use blake2::Digest;

use crate::target_dir;
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
    hex::encode(blake2::Blake2s256::digest(bytes))
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
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn sri(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let d = sha2::Sha384::digest(bytes);
    format!(
        "sha384-{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, d)
    )
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
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<meta name=\"referrer\" content=\"no-referrer\">\n<title>burrow admin</title>\n<link rel=\"icon\" href=\"data:,\">\n<link rel=\"stylesheet\" href=\"/{css}\" integrity=\"{css_sri}\">\n<script type=\"module\" src=\"/{boot}\" integrity=\"{boot_sri}\"></script>\n</head>\n<body><p id=\"boot\" class=\"p-8 text-sm text-text-muted\">Loading burrow admin…</p></body>\n</html>\n"
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
    let wasm = compile_wasm(root, release)?;
    let work = target_dir(root).join("admin-ui-work");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let bg = bindgen(tools, &work, &wasm, release)?;
    let css_tmp = work.join("app.css");
    tailwind(tools, &ui, &css_tmp, release)?;

    let dist = dist_dir(root);
    prepare_dist(&dist, release)?;
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let wasm_bytes = read(&bg)?;
    let wasm_name = hashed("app.wasm", &wasm_bytes, release);
    write(&dist, &wasm_name, &wasm_bytes)?;
    let js_bytes = read(&work.join("app.js"))?;
    let js_name = hashed("app.js", &js_bytes, release);
    write(&dist, &js_name, &js_bytes)?;
    let mut css_text = String::from_utf8(read(&css_tmp)?).map_err(|e| e.to_string())?;
    let extra = write_fonts(&ui, &dist, &mut css_text, release)?;
    if css_text.contains("/fonts/") {
        return Err("app.css refers to a font that is not in assets/fonts".into());
    }
    let css_bytes = css_text.into_bytes();
    let css_name = hashed("app.css", &css_bytes, release);
    write(&dist, &css_name, &css_bytes)?;
    let boot = format!(
        "import init from \"./{js_name}\";\ninit({{ module_or_path: new URL(\"./{wasm_name}\", import.meta.url) }});\n"
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

    let files = files_of(&boot_name, &js_name, &wasm_name, &css_name, &extra);
    write_manifest(&ui, &dist, &files, release)?;
    let _ = std::fs::remove_dir_all(&work);
    println!("admin UI written to {}", dist.display());
    Ok(dist)
}

fn compile_wasm(root: &Path, release: bool) -> Result<PathBuf, String> {
    let mut cargo = crate::cargo();
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
    Ok(wasm)
}

fn bindgen(tools: &Tools, work: &Path, wasm: &Path, release: bool) -> Result<PathBuf, String> {
    run(Command::new(&tools.wasm_bindgen)
        .args([
            "--target",
            "web",
            "--no-typescript",
            "--out-name",
            "app",
            "--out-dir",
        ])
        .arg(work)
        .arg(wasm))?;
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
    Ok(bg)
}

fn tailwind(tools: &Tools, ui: &Path, css_tmp: &Path, release: bool) -> Result<(), String> {
    let mut tw = Command::new(&tools.tailwind);
    tw.arg("-i")
        .arg(ui.join("style/entry.css"))
        .arg("-o")
        .arg(css_tmp)
        .current_dir(ui);
    if release {
        tw.arg("--minify");
    }
    run(&mut tw)?;
    Ok(())
}

fn prepare_dist(dist: &Path, release: bool) -> Result<(), String> {
    if release {
        let _ = std::fs::remove_dir_all(dist);
    } else if let Ok(rd) = std::fs::read_dir(dist) {
        for e in rd.flatten().filter(|e| e.file_name() != "app.css") {
            let _ = std::fs::remove_file(e.path());
        }
    }
    std::fs::create_dir_all(dist).map_err(|e| e.to_string())?;
    Ok(())
}

fn write_fonts(
    ui: &Path,
    dist: &Path,
    css_text: &mut String,
    release: bool,
) -> Result<Vec<String>, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let mut extra = Vec::new();
    let fonts = ui.join("assets/fonts");
    let mut font_files: Vec<_> = std::fs::read_dir(&fonts)
        .map_err(|e| format!("{}: {e}", fonts.display()))?
        .flatten()
        .map(|e| e.path())
        .collect();
    font_files.sort();
    for p in font_files {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let bytes = read(&p)?;
        let out = if name.ends_with(".woff2") {
            let h = hashed(&name, &bytes, release);
            *css_text = css_text.replace(&format!("/fonts/{name}"), &format!("./{h}"));
            h
        } else if name.starts_with("LICENSE") {
            "fonts-license.txt".to_string()
        } else {
            continue;
        };
        write(dist, &out, &bytes)?;
        extra.push(out);
    }
    Ok(extra)
}

fn files_of(
    boot_name: &str,
    js_name: &str,
    wasm_name: &str,
    css_name: &str,
    extra: &[String],
) -> Vec<File> {
    let mut files = vec![
        File {
            path: "/index.html".into(),
            file: "index.html".into(),
            mime: mime("index.html"),
        },
        File {
            path: format!("/{boot_name}"),
            file: boot_name.to_string(),
            mime: mime(boot_name),
        },
        File {
            path: format!("/{js_name}"),
            file: js_name.to_string(),
            mime: mime(js_name),
        },
        File {
            path: format!("/{wasm_name}"),
            file: wasm_name.to_string(),
            mime: mime(wasm_name),
        },
        File {
            path: format!("/{css_name}"),
            file: css_name.to_string(),
            mime: mime(css_name),
        },
    ];
    for name in extra {
        files.push(File {
            path: format!("/{name}"),
            file: name.clone(),
            mime: mime(name),
        });
    }
    files
}

fn write_manifest(ui: &Path, dist: &Path, files: &[File], release: bool) -> Result<(), String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let mut listed = Vec::new();
    for f in files {
        let bytes = read(&dist.join(&f.file))?;
        let gz_size = if f.file.ends_with(".woff2") {
            bytes.len() as u64
        } else if release {
            gzip(dist, &f.file)?;
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
        dist,
        "manifest.json",
        serde_json::to_string_pretty(&manifest)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )?;
    Ok(())
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
        assert_eq!(
            sri(b""),
            "sha384-OLBgp1GsljhM2TJ+sbHjaiH9txEUvgdDTAzHv2P24donTt6/529l+9Ua0vFImLlb"
        );
        assert_eq!(
            sri(b"f"),
            "sha384-OjHKRDtsrjcXtKTxmXK8kkE2RTgOSZDk/k3DIjhklPraorY7T2K+bitwd+mCu3ra"
        );
        assert_eq!(
            sri(b"foo"),
            "sha384-mMEf/f3VQGdrGhN8saIrKnA1DJpEFx1rEYDGvly7LuP3nVMsih3Z7y6OCOdSo7q7"
        );
    }

    #[test]
    fn the_page_has_no_inline_script_or_style() {
        let html = index_html("app-1.css", "sha384-a", "boot-2.js", "sha384-b");
        assert!(html.contains(
            "<script type=\"module\" src=\"/boot-2.js\" integrity=\"sha384-b\"></script>"
        ));
        assert!(!html.contains("<style"));
        assert_eq!(html.matches("<script").count(), 1);
        assert!(
            html.contains(
                "<p id=\"boot\" class=\"p-8 text-sm text-text-muted\">Loading burrow admin…</p>"
            ),
            "the page says it is loading before the WebAssembly starts"
        );
        assert_eq!(hashed("app.js", b"x", false), "app.js");
        assert!(hashed("app.js", b"x", true).starts_with("app-"));
    }
}
