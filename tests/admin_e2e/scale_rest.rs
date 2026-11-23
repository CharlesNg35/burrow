use std::time::Duration;

use super::edits::Flow;
use super::world::{Options, secret};
use super::{Ctx, SECS, open, run};

const SERVICES: usize = 5000;
const CLIENTS: usize = 500;
const VISITORS: usize = 1000;
const CHANGED: usize = 1000;
const LONG: Duration = Duration::from_secs(120);

fn opts() -> Options {
    let services = (0..SERVICES)
        .map(|i| {
            format!(
                "  svc{i:04}:\n    client: box{:03}\n    target: {}\n    visitors: [laptop]\n",
                i % CLIENTS,
                10000 + i
            )
        })
        .collect();
    let clients = (0..CLIENTS)
        .map(|i| format!("  box{i:03}:\n    secret: {}\n", secret()))
        .collect();
    let visitors: String = std::iter::once("visitors:\n".to_string())
        .chain((0..VISITORS).map(|i| format!("  guest{i:04}:\n    secret: {}\n", secret())))
        .collect();
    Options {
        extra_services: services,
        extra_clients: clients,
        drop_ins: vec![("guests.yaml", visitors, false)],
        ..Options::default()
    }
}

const TIMED: &str = "(async () => { const t = performance.now(); ACT; await new Promise(r => requestAnimationFrame(() => setTimeout(r, 0))); return performance.now() - t; })()";

async fn timed(c: &Ctx, act: &str) -> f64 {
    c.ui.eval(&TIMED.replace("ACT", act)).await
}

async fn until(c: &Ctx, act: &str, css: &str) -> f64 {
    c.ui.eval(&format!(
        "(async () => {{ const t = performance.now(); {act}; while (!document.querySelector('{css}')) await new Promise(r => requestAnimationFrame(r)); await new Promise(r => requestAnimationFrame(() => setTimeout(r, 0))); return performance.now() - t; }})()"
    ))
    .await
}

const TYPE: &str = "const i = document.querySelector('[data-testid=search] input'); i.value = 'TEXT'; i.dispatchEvent(new Event('input', { bubbles: true }))";

async fn search(c: &Ctx, out: &mut Vec<String>) {
    c.ui.click("[data-testid=search-open]").await;
    c.ui.wait_for("[data-testid=search] input", SECS).await;
    let mut typed = Vec::new();
    for text in ["s", "sv", "svc", "svc0", "svc00", "svc004", "svc0042"] {
        typed.push(timed(c, &TYPE.replace("TEXT", text)).await.round());
    }
    let options = c.ui.count("[data-testid=search] [role=option]").await;
    let first: String = c
        .ui
        .eval("document.querySelector('[data-testid=search] [role=option]')?.textContent ?? ''")
        .await;
    out.push(format!(
        "search over {SERVICES} services, {CLIENTS} clients, {VISITORS} visitors: typing {typed:?} ms; {options} options for svc0042; first: {first}"
    ));
    let wide = timed(c, &TYPE.replace("TEXT", "e")).await.round();
    let wide_options = c.ui.count("[data-testid=search] [role=option]").await;
    out.push(format!(
        "search for 'e' (matches most names): {wide} ms, {wide_options} options"
    ));
    c.ui.press("[data-testid=search]", "Escape").await;
    c.ui.wait_gone("[data-testid=search]", SECS).await;
}

async fn first_row(c: &Ctx) -> String {
    c.ui.nav("/activity").await;
    c.ui.wait_for("[data-testid=activity] [data-row]", SECS)
        .await;
    c.ui.all_attrs("[data-testid=activity] [data-row]", "data-row")
        .await
        .into_iter()
        .next()
        .expect("an activity entry")
}

async fn version_diff(c: &Ctx, out: &mut Vec<String>) {
    let before = first_row(c).await;
    c.world
        .hand_edit(|t| t.replace("    target: 10", "    target: 30"));
    c.ui.wait_true(
        &format!(
            "(() => {{ const r = document.querySelector('[data-testid=activity] [data-row]'); return !!r && r.dataset.row !== '{before}'; }})()"
        ),
        "the hand edit shows in Activity",
        LONG,
    )
    .await;
    let rev = first_row(c).await;
    c.ui.nav("/overview").await;
    c.ui.wait_gone("[data-testid=activity]", SECS).await;
    let ms = until(
        c,
        &format!("const run = (location.pathname.match(/^\\/r-[0-9a-f]+/) || [''])[0]; history.pushState(null, '', run + '/activity/{rev}'); dispatchEvent(new PopStateEvent('popstate'))"),
        "[data-testid=activity-diff] [role=row]",
    )
    .await
    .round();
    let rows = c.ui.count("[data-testid=activity-diff] [role=row]").await;
    let nodes: usize =
        c.ui.eval("document.querySelectorAll('[data-testid=activity-diff] *').length")
            .await;
    out.push(format!(
        "version diff of {CHANGED} changed services: shown in {ms} ms; {rows} rows and {nodes} elements in the DOM"
    ));
}

async fn revert_review(c: &Ctx, out: &mut Vec<String>) {
    c.ui.wait_for("[data-testid=revert]:not([disabled])", SECS)
        .await;
    let ms = until(
        c,
        "document.querySelector('[data-testid=revert]').click()",
        "[data-testid=review]",
    )
    .await
    .round();
    let paths = c.ui.count("[data-testid=review-path-line]").await;
    let svgs = c.ui.count("[data-testid=review-paths] svg").await;
    let rows = c.ui.count("[data-testid=review-diff] [role=row]").await;
    let nodes: usize = c.ui.eval("document.querySelectorAll('*').length").await;
    out.push(format!(
        "review of reverting {CHANGED} services: shown in {ms} ms; {paths} service lines, {svgs} svg elements in the service paths, {rows} diff rows, {nodes} elements on the page"
    ));
}

fn scale_rest(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.viewport(1440, 900).await;
        c.ui.wait_for("[data-testid=search-open]", SECS).await;
        let mut out = Vec::new();
        search(c, &mut out).await;
        version_diff(c, &mut out).await;
        revert_review(c, &mut out).await;
        let text = out.join("\n") + "\n";
        println!("{text}");
        let dir = super::sheet::shots_dir().join("scale-rest");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("timings.txt"), text);
        c.ui.assert_no_errors();
    })
}

#[test]
fn search_review_and_diffs_stay_fast_at_scale() {
    run("scale-rest", opts(), false, scale_rest);
}
