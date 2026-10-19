use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::Digest;

const BINARYEN: &str = "version_133";
const BINARYEN_SHA256: &str = "2dc9c7813f5375db93d96ead4b78222fcc3e2677bbb832297af4797782a37489";

pub fn run(root: &Path) -> Result<PathBuf, String> {
    let dir = root.join(".tools");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let wasm_opt = dir.join(format!("binaryen-{BINARYEN}/bin/wasm-opt"));
    if !wasm_opt.exists() {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Err("cargo xtask tools downloads binaryen for x86_64 Linux only; install wasm-opt and put it on PATH".into());
        }
        let name = format!("binaryen-{BINARYEN}-x86_64-linux.tar.gz");
        let url =
            format!("https://github.com/WebAssembly/binaryen/releases/download/{BINARYEN}/{name}");
        let tarball = dir.join(&name);
        let status = Command::new("curl")
            .args(["-fsSL", "--retry", "3", "-o"])
            .arg(&tarball)
            .arg(&url)
            .status()
            .map_err(|e| format!("curl: {e}"))?;
        if !status.success() {
            return Err(format!("downloading {url} failed"));
        }
        let bytes = std::fs::read(&tarball).map_err(|e| e.to_string())?;
        let got: String = sha2::Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if got != BINARYEN_SHA256 {
            let _ = std::fs::remove_file(&tarball);
            return Err(format!(
                "{name} has sha256 {got}, expected {BINARYEN_SHA256}"
            ));
        }
        let status = Command::new("tar")
            .arg("xzf")
            .arg(&tarball)
            .arg("-C")
            .arg(&dir)
            .status()
            .map_err(|e| format!("tar: {e}"))?;
        let _ = std::fs::remove_file(&tarball);
        if !status.success() || !wasm_opt.exists() {
            return Err(format!("unpacking {name} failed"));
        }
    }
    let version = Command::new(&wasm_opt)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    println!("wasm-opt: {} ({version})", wasm_opt.display());
    Ok(wasm_opt)
}
