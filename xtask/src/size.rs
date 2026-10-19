use std::path::Path;

pub const WASM_GZ_BUDGET: u64 = 350 * 1024;
pub const WASM_GZ_WARN: u64 = 300 * 1024;
pub const FONTS_BUDGET: u64 = 55 * 1024;
pub const OTHER_GZ_BUDGET: u64 = 30 * 1024;
pub const TOTAL_GZ_BUDGET: u64 = 400 * 1024;

#[derive(Debug, Default, PartialEq)]
pub struct Sizes {
    pub wasm: u64,
    pub fonts: u64,
    pub other: u64,
}

impl Sizes {
    pub fn total_without_fonts(&self) -> u64 {
        self.wasm + self.other
    }

    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.wasm > WASM_GZ_BUDGET {
            out.push(format!("wasm {} B > {WASM_GZ_BUDGET} B", self.wasm));
        }
        if self.fonts > FONTS_BUDGET {
            out.push(format!("fonts {} B > {FONTS_BUDGET} B", self.fonts));
        }
        if self.other > OTHER_GZ_BUDGET {
            out.push(format!("other {} B > {OTHER_GZ_BUDGET} B", self.other));
        }
        if self.total_without_fonts() > TOTAL_GZ_BUDGET {
            out.push(format!(
                "total without fonts {} B > {TOTAL_GZ_BUDGET} B",
                self.total_without_fonts()
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
    println!(
        "fonts {} B (budget {FONTS_BUDGET}, woff2 does not compress)",
        s.fonts
    );
    println!("other gzip {} B (budget {OTHER_GZ_BUDGET})", s.other);
    println!(
        "total {} B with fonts, {} B without (budget {TOTAL_GZ_BUDGET} without fonts)",
        s.total_without_fonts() + s.fonts,
        s.total_without_fonts()
    );
    if s.wasm > WASM_GZ_WARN {
        println!("warning: the wasm is above {WASM_GZ_WARN} B");
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
        let big = Sizes {
            wasm: WASM_GZ_BUDGET + 1,
            fonts: 0,
            other: 0,
        };
        assert_eq!(big.problems().len(), 1);
    }
}
