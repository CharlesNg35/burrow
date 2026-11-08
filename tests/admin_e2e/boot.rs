use super::world::Options;
use super::{Ctx, SECS, run};

const NO_SIMD: &str = "(() => { const v = WebAssembly.validate; WebAssembly.validate = function (b) { const a = new Uint8Array(b); if (a.length === 31 && a[a.length - 2] === 98) return false; return v.apply(this, arguments); }; })()";
const WASM_FETCHED: &str =
    "performance.getEntriesByType('resource').filter(e => e.name.endsWith('.wasm')).length";

fn no_simd(c: &mut Ctx) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + '_>> {
    Box::pin(async move {
        c.ui.init_script(NO_SIMD).await;
        let url = c.world.open_admin("ops");
        c.ui.goto(&url).await;
        c.ui.wait_for("#boot[data-unsupported=simd][role=alert]", SECS)
            .await;
        let text = c.ui.text("#boot").await;
        assert_eq!(
            text,
            "burrow admin needs a newer browser: Chrome or Edge 91 or later, Firefox 89 or later, or Safari 16.4 or later."
        );
        let fetched: u64 = c.ui.eval(WASM_FETCHED).await;
        assert_eq!(fetched, 0, "no WebAssembly is fetched where it cannot run");
        let png = c.ui.png().await;
        c.ui.flow.save("boot-needs-simd", &png);
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_browser_without_webassembly_simd_is_told_which_browsers_work() {
    run("boot-simd", Options::default(), false, no_simd);
}
