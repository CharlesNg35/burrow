use std::path::Path;

pub const WASM_GZ_BUDGET: u64 = 1_048_576;
pub const TOTAL_BUDGET: u64 = 1_258_291;

pub fn warn_at(budget: u64) -> u64 {
    budget / 10 * 9
}

#[derive(Debug, Default, PartialEq)]
pub struct Sizes {
    pub wasm: u64,
    pub fonts: u64,
    pub other: u64,
}

impl Sizes {
    pub fn total(&self) -> u64 {
        self.wasm + self.fonts + self.other
    }

    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.wasm > WASM_GZ_BUDGET {
            out.push(format!("wasm {} B > {WASM_GZ_BUDGET} B", self.wasm));
        }
        if self.total() > TOTAL_BUDGET {
            out.push(format!(
                "all UI assets {} B > {TOTAL_BUDGET} B",
                self.total()
            ));
        }
        out
    }

    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.wasm > warn_at(WASM_GZ_BUDGET) {
            out.push(format!(
                "the wasm is above 90 % of its budget ({} B)",
                warn_at(WASM_GZ_BUDGET)
            ));
        }
        if self.total() > warn_at(TOTAL_BUDGET) {
            out.push(format!(
                "all UI assets are above 90 % of their budget ({} B)",
                warn_at(TOTAL_BUDGET)
            ));
        }
        out
    }
}

pub fn measure(manifest: &serde_json::Value) -> Sizes {
    let mut s = Sizes::default();
    for f in manifest["files"].as_array().into_iter().flatten() {
        let gz = f["gz_size"].as_u64().unwrap_or(0);
        let name = f["path"].as_str().unwrap_or_default();
        if name.ends_with(".wasm") {
            s.wasm += gz;
        } else if name.ends_with(".woff2") {
            s.fonts += gz;
        } else {
            s.other += gz;
        }
    }
    s
}

pub fn run(root: &Path, budget: bool) -> Result<(), String> {
    let dist = std::env::var_os("BURROW_ADMIN_UI_DIST")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("crates/burrow-admin-ui/dist"));
    let manifest = dist.join("manifest.json");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("{}: {e}; run cargo xtask ui first", manifest.display()))?;
    let m: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    for f in m["files"].as_array().into_iter().flatten() {
        println!(
            "{:40} {:>9} B raw {:>9} B gzip",
            f["path"].as_str().unwrap_or_default(),
            f["size"].as_u64().unwrap_or(0),
            f["gz_size"].as_u64().unwrap_or(0)
        );
    }
    let s = measure(&m);
    println!("wasm gzip {} B (budget {WASM_GZ_BUDGET})", s.wasm);
    println!("fonts {} B (woff2 does not compress)", s.fonts);
    println!("other gzip {} B", s.other);
    println!(
        "all UI assets {} B, fonts included (budget {TOTAL_BUDGET})",
        s.total()
    );
    for w in s.warnings() {
        println!("warning: {w}");
    }
    let problems = s.problems();
    if budget && !problems.is_empty() {
        return Err(format!("size budget exceeded: {}", problems.join("; ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_are_counted_on_their_own_line() {
        let m = serde_json::json!({"files": [
            {"path": "/app.wasm", "gz_size": 100},
            {"path": "/geist.woff2", "gz_size": 30},
            {"path": "/app.css", "gz_size": 5},
            {"path": "/index.html", "gz_size": 1}
        ]});
        let s = measure(&m);
        assert_eq!(
            s,
            Sizes {
                wasm: 100,
                fonts: 30,
                other: 6
            }
        );
        assert!(s.problems().is_empty());
        assert!(s.warnings().is_empty());
        let big = Sizes {
            wasm: WASM_GZ_BUDGET + 1,
            fonts: 0,
            other: 0,
        };
        assert_eq!(big.problems().len(), 1);
        let fonts_tip_it = Sizes {
            wasm: WASM_GZ_BUDGET,
            fonts: TOTAL_BUDGET - WASM_GZ_BUDGET + 1,
            other: 0,
        };
        assert_eq!(
            fonts_tip_it.problems().len(),
            1,
            "fonts count toward the total"
        );
        let near = Sizes {
            wasm: warn_at(WASM_GZ_BUDGET) + 1,
            fonts: 0,
            other: 0,
        };
        assert!(near.problems().is_empty());
        assert_eq!(near.warnings().len(), 1);
        assert_eq!(warn_at(1000), 900);
    }
}
