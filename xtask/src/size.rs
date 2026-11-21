use std::path::Path;

pub const WASM_GZ_BUDGET: u64 = 2_097_152;
pub const WASM_TOTAL_BUDGET: u64 = 3_670_016;
pub const ASSETS_ROOM: u64 = 419_430;
pub const TOTAL_BUDGET: u64 = WASM_TOTAL_BUDGET + ASSETS_ROOM;

pub fn warn_at(budget: u64) -> u64 {
    budget / 10 * 9
}

#[derive(Debug, Default, PartialEq)]
pub struct Sizes {
    pub wasm: u64,
    pub lazy: u64,
    pub fonts: u64,
    pub other: u64,
}

impl Sizes {
    pub fn all_wasm(&self) -> u64 {
        self.wasm + self.lazy
    }

    pub fn total(&self) -> u64 {
        self.all_wasm() + self.fonts + self.other
    }

    fn checks(&self) -> [(&'static str, u64, u64); 3] {
        [
            ("the main wasm", self.wasm, WASM_GZ_BUDGET),
            (
                "all wasm, lazy chunks included",
                self.all_wasm(),
                WASM_TOTAL_BUDGET,
            ),
            ("all UI assets", self.total(), TOTAL_BUDGET),
        ]
    }

    pub fn problems(&self) -> Vec<String> {
        self.checks()
            .into_iter()
            .filter(|(_, size, budget)| size > budget)
            .map(|(what, size, budget)| format!("{what} {size} B > {budget} B"))
            .collect()
    }

    pub fn warnings(&self) -> Vec<String> {
        self.checks()
            .into_iter()
            .filter(|(_, size, budget)| *size > warn_at(*budget))
            .map(|(what, _, budget)| {
                format!("{what} is above 90 % of its budget ({} B)", warn_at(budget))
            })
            .collect()
    }
}

pub fn measure(manifest: &serde_json::Value) -> Sizes {
    let mut s = Sizes::default();
    for f in manifest["files"].as_array().into_iter().flatten() {
        let gz = f["gz_size"].as_u64().unwrap_or(0);
        let name = f["path"].as_str().unwrap_or_default();
        if name.ends_with(".wasm") && f["lazy"].as_bool() == Some(true) {
            s.lazy += gz;
        } else if name.ends_with(".wasm") {
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
    println!("main wasm gzip {} B (budget {WASM_GZ_BUDGET})", s.wasm);
    println!("lazy wasm chunks gzip {} B", s.lazy);
    println!(
        "all wasm gzip {} B (budget {WASM_TOTAL_BUDGET})",
        s.all_wasm()
    );
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

    fn sizes(wasm: u64, lazy: u64, fonts: u64) -> Sizes {
        Sizes {
            wasm,
            lazy,
            fonts,
            other: 0,
        }
    }

    #[test]
    fn main_wasm_lazy_chunks_fonts_and_the_rest_are_counted_apart() {
        let m = serde_json::json!({"files": [
            {"path": "/app.wasm", "gz_size": 100, "lazy": false},
            {"path": "/split_editor.wasm", "gz_size": 40, "lazy": true},
            {"path": "/chunk_3.wasm", "gz_size": 2, "lazy": true},
            {"path": "/geist.woff2", "gz_size": 30},
            {"path": "/app.css", "gz_size": 5},
            {"path": "/__wasm_split.x.js", "gz_size": 3, "lazy": false},
            {"path": "/index.html", "gz_size": 1}
        ]});
        let s = measure(&m);
        assert_eq!(
            s,
            Sizes {
                wasm: 100,
                lazy: 42,
                fonts: 30,
                other: 9
            }
        );
        assert_eq!(s.all_wasm(), 142);
        assert_eq!(s.total(), 181);
        assert!(s.problems().is_empty());
        assert!(s.warnings().is_empty());
        let older = serde_json::json!({"files": [{"path": "/app.wasm", "gz_size": 7}]});
        assert_eq!(
            measure(&older).wasm,
            7,
            "a manifest without lazy marks is all main"
        );
    }

    #[test]
    fn the_main_wasm_keeps_its_own_budget_and_lazy_chunks_share_the_wasm_total() {
        assert_eq!(WASM_GZ_BUDGET, 2 * 1024 * 1024);
        assert_eq!(
            WASM_TOTAL_BUDGET,
            7 * 1024 * 1024 / 2,
            "3.5 MiB of wasm in all"
        );
        assert_eq!(TOTAL_BUDGET - WASM_TOTAL_BUDGET, ASSETS_ROOM);
        assert_eq!(sizes(WASM_GZ_BUDGET + 1, 0, 0).problems().len(), 1);
        let lazy_fits = sizes(WASM_GZ_BUDGET, WASM_TOTAL_BUDGET - WASM_GZ_BUDGET, 0);
        assert!(
            lazy_fits.problems().is_empty(),
            "{:?}",
            lazy_fits.problems()
        );
        let lazy_over = sizes(WASM_GZ_BUDGET, WASM_TOTAL_BUDGET - WASM_GZ_BUDGET + 1, 0);
        assert_eq!(
            lazy_over.problems(),
            vec![format!(
                "all wasm, lazy chunks included {} B > {WASM_TOTAL_BUDGET} B",
                WASM_TOTAL_BUDGET + 1
            )]
        );
        let small_main_big_chunk = sizes(1, WASM_TOTAL_BUDGET - 1, 0);
        assert!(small_main_big_chunk.problems().is_empty());
    }

    #[test]
    fn fonts_count_toward_the_total_only() {
        let room = sizes(
            WASM_GZ_BUDGET,
            WASM_TOTAL_BUDGET - WASM_GZ_BUDGET,
            ASSETS_ROOM,
        );
        assert!(room.problems().is_empty());
        let tip = sizes(
            WASM_GZ_BUDGET,
            WASM_TOTAL_BUDGET - WASM_GZ_BUDGET,
            ASSETS_ROOM + 1,
        );
        assert_eq!(tip.problems().len(), 1, "fonts count toward the total");
        assert!(tip.problems()[0].starts_with("all UI assets"));
    }

    #[test]
    fn each_budget_warns_above_90_percent() {
        assert_eq!(warn_at(1000), 900);
        let near_main = sizes(warn_at(WASM_GZ_BUDGET) + 1, 0, 0);
        assert!(near_main.problems().is_empty());
        assert_eq!(near_main.warnings().len(), 1);
        assert!(near_main.warnings()[0].starts_with("the main wasm"));
        let near_all = sizes(
            warn_at(WASM_GZ_BUDGET),
            warn_at(WASM_TOTAL_BUDGET) - warn_at(WASM_GZ_BUDGET) + 1,
            0,
        );
        assert_eq!(
            near_all.warnings(),
            vec![format!(
                "all wasm, lazy chunks included is above 90 % of its budget ({} B)",
                warn_at(WASM_TOTAL_BUDGET)
            )]
        );
    }
}
