use std::path::Path;

pub const WASM_GZ_BUDGET: u64 = 350 * 1024;
pub const WASM_GZ_WARN: u64 = 300 * 1024;
pub const OTHER_GZ_BUDGET: u64 = 30 * 1024;
pub const TOTAL_GZ_BUDGET: u64 = 400 * 1024;

pub fn run(root: &Path, budget: bool) -> Result<(), String> {
    let dist = std::env::var_os("BURROW_ADMIN_UI_DIST")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("crates/burrow-admin-ui/dist"));
    let manifest = dist.join("manifest.json");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("{}: {e}; run cargo xtask ui first", manifest.display()))?;
    let m: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut wasm = 0;
    let mut other = 0;
    let empty = Vec::new();
    for f in m["files"].as_array().unwrap_or(&empty) {
        let gz = f["gz_size"].as_u64().unwrap_or(0);
        let name = f["path"].as_str().unwrap_or_default();
        println!(
            "{name:40} {:>9} B raw {:>9} B gzip",
            f["size"].as_u64().unwrap_or(0),
            gz
        );
        if name.ends_with(".wasm") {
            wasm += gz;
        } else {
            other += gz;
        }
    }
    let total = wasm + other;
    println!(
        "wasm gzip {wasm} B (budget {WASM_GZ_BUDGET}), other gzip {other} B (budget {OTHER_GZ_BUDGET}), total {total} B (budget {TOTAL_GZ_BUDGET})"
    );
    if wasm > WASM_GZ_WARN {
        println!("warning: the wasm is above {WASM_GZ_WARN} B");
    }
    if budget && (wasm > WASM_GZ_BUDGET || other > OTHER_GZ_BUDGET || total > TOTAL_GZ_BUDGET) {
        return Err("size budget exceeded".into());
    }
    Ok(())
}
