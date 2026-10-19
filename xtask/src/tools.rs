use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::Digest;

const BINARYEN: &str = "version_133";
const BINARYEN_SHA256: &str = "2dc9c7813f5375db93d96ead4b78222fcc3e2677bbb832297af4797782a37489";

const TAILWIND: &str = "v4.3.3";
const TAILWIND_SHA256: &[(&str, &str)] = &[
    (
        "tailwindcss-linux-x64",
        "dc61b3ac6b8c9ca874c0cc4c57b2409791a64c5540404ca5f5367360babc313a",
    ),
    (
        "tailwindcss-linux-arm64",
        "55fd0b241214eff3de1e8ee4f22796662f2d2e7a49bcfca7477cfd0bac398195",
    ),
    (
        "tailwindcss-macos-x64",
        "7922e0953f2110c05976e3bf58f14e643d90427575e766b7d433f5f80cbee7e1",
    ),
    (
        "tailwindcss-macos-arm64",
        "cdf646702987a743464dff4d9c60fd4480d1c1e73dd819a9a67f1078815dce9d",
    ),
    (
        "tailwindcss-windows-x64.exe",
        "e0e260ce048014e9268f6237ff18f8ccf02cef521cbd0ae04e82c2cdf7aa3955",
    ),
];

pub struct Tools {
    pub wasm_opt: PathBuf,
    pub tailwind: PathBuf,
    pub wasm_bindgen: PathBuf,
}

fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn fetch(url: &str, to: &Path, sha256: &str) -> Result<(), String> {
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(to)
        .arg(url)
        .status()
        .map_err(|e| format!("curl: {e}"))?;
    if !status.success() {
        return Err(format!("downloading {url} failed"));
    }
    let bytes = std::fs::read(to).map_err(|e| e.to_string())?;
    let got = sha256_hex(&bytes);
    if got != sha256 {
        let _ = std::fs::remove_file(to);
        return Err(format!(
            "{} has sha256 {got}, expected {sha256}",
            to.display()
        ));
    }
    Ok(())
}

fn binaryen(dir: &Path) -> Result<PathBuf, String> {
    let wasm_opt = dir.join(format!("binaryen-{BINARYEN}/bin/wasm-opt"));
    if wasm_opt.exists() {
        return Ok(wasm_opt);
    }
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err("cargo xtask tools downloads binaryen for x86_64 Linux only; install wasm-opt and put it on PATH".into());
    }
    let name = format!("binaryen-{BINARYEN}-x86_64-linux.tar.gz");
    let url =
        format!("https://github.com/WebAssembly/binaryen/releases/download/{BINARYEN}/{name}");
    let tarball = dir.join(&name);
    fetch(&url, &tarball, BINARYEN_SHA256)?;
    let status = Command::new("tar")
        .arg("xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(dir)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    let _ = std::fs::remove_file(&tarball);
    if !status.success() || !wasm_opt.exists() {
        return Err(format!("unpacking {name} failed"));
    }
    Ok(wasm_opt)
}

fn tailwind_asset() -> Option<&'static str> {
    let name = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "tailwindcss-linux-x64",
        ("linux", "aarch64") => "tailwindcss-linux-arm64",
        ("macos", "x86_64") => "tailwindcss-macos-x64",
        ("macos", "aarch64") => "tailwindcss-macos-arm64",
        ("windows", "x86_64") => "tailwindcss-windows-x64.exe",
        _ => return None,
    };
    Some(name)
}

fn tailwind(dir: &Path) -> Result<PathBuf, String> {
    let asset = tailwind_asset()
        .ok_or("no Tailwind standalone build for this host; put tailwindcss on PATH")?;
    let sha = TAILWIND_SHA256
        .iter()
        .find(|(n, _)| *n == asset)
        .map(|(_, s)| *s)
        .ok_or("no pinned sha256 for this Tailwind build")?;
    let bin = dir.join(format!("tailwindcss-{TAILWIND}/{asset}"));
    if bin.exists() {
        return Ok(bin);
    }
    std::fs::create_dir_all(bin.parent().unwrap_or(dir)).map_err(|e| e.to_string())?;
    let url =
        format!("https://github.com/tailwindlabs/tailwindcss/releases/download/{TAILWIND}/{asset}");
    fetch(&url, &bin, sha)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    Ok(bin)
}

pub fn locked_version(root: &Path, krate: &str) -> Result<String, String> {
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).map_err(|e| e.to_string())?;
    let needle = format!("name = \"{krate}\"\nversion = \"");
    let at = lock
        .find(&needle)
        .ok_or_else(|| format!("{krate} is not in Cargo.lock"))?;
    let rest = &lock[at + needle.len()..];
    Ok(rest[..rest.find('"').unwrap_or(0)].to_string())
}

fn wasm_bindgen(root: &Path, dir: &Path) -> Result<PathBuf, String> {
    let version = locked_version(root, "wasm-bindgen")?;
    let home = dir.join(format!("wasm-bindgen-{version}"));
    let bin = home.join("bin").join(if cfg!(windows) {
        "wasm-bindgen.exe"
    } else {
        "wasm-bindgen"
    });
    if !bin.exists() {
        let status = Command::new("cargo")
            .args(["binstall", "--no-confirm", "--force", "--root"])
            .arg(&home)
            .arg(format!("wasm-bindgen-cli@{version}"))
            .status()
            .map_err(|e| format!("cargo binstall: {e}; install cargo-binstall first"))?;
        if !status.success() || !bin.exists() {
            return Err(format!("installing wasm-bindgen-cli {version} failed"));
        }
    }
    let out = Command::new(&bin)
        .arg("--version")
        .output()
        .map_err(|e| e.to_string())?;
    let shown = String::from_utf8_lossy(&out.stdout);
    if !shown.contains(&version) {
        return Err(format!(
            "{} reports {}, but Cargo.lock has wasm-bindgen {version}",
            bin.display(),
            shown.trim()
        ));
    }
    Ok(bin)
}

pub fn run(root: &Path) -> Result<Tools, String> {
    let dir = root.join(".tools");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tools = Tools {
        wasm_opt: binaryen(&dir)?,
        tailwind: tailwind(&dir)?,
        wasm_bindgen: wasm_bindgen(root, &dir)?,
    };
    println!("wasm-opt: {}", tools.wasm_opt.display());
    println!("tailwindcss {TAILWIND}: {}", tools.tailwind.display());
    println!("wasm-bindgen: {}", tools.wasm_bindgen.display());
    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tailwind_build_has_a_pin() {
        assert_eq!(TAILWIND_SHA256.len(), 5);
        assert!(TAILWIND_SHA256.iter().all(|(_, s)| s.len() == 64));
        if let Some(a) = tailwind_asset() {
            assert!(TAILWIND_SHA256.iter().any(|(n, _)| *n == a));
        }
    }

    #[test]
    fn versions_come_from_the_lock_file() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("root");
        let v = locked_version(root, "wasm-bindgen").expect("locked");
        assert!(v.starts_with("0.2."), "{v}");
    }
}
