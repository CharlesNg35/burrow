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

fn save(out: &mut Vec<String>, line: String) {
    println!("{line}");
    out.push(line);
    let dir = super::sheet::shots_dir().join("scale-rest");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("timings.txt"), out.join("\n") + "\n");
}

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
    save(
        out,
        format!(
            "search over {SERVICES} services, {CLIENTS} clients, {VISITORS} visitors: typing {typed:?} ms; {options} options for svc0042; first: {first}"
        ),
    );
    assert!(
        first.starts_with("svc0042"),
        "the exact name comes first: {first}"
    );
    assert!(options <= 40, "the results stay short: {options}");
    let mut sorted = typed.clone();
    sorted.sort_by(f64::total_cmp);
    assert!(
        sorted[sorted.len() / 2] < 200.0 && sorted.iter().all(|ms| *ms < 2000.0),
        "each keystroke answers quickly: {typed:?}"
    );
    let wide = timed(c, &TYPE.replace("TEXT", "e")).await.round();
    let wide_options = c.ui.count("[data-testid=search] [role=option]").await;
    save(
        out,
        format!("search for 'e' (matches most names): {wide} ms, {wide_options} options"),
    );
    show_all(c).await;
}

async fn keys(c: &Ctx) {
    let input = "[data-testid=search] input";
    assert_eq!(
        c.ui.attr_of(input, "aria-activedescendant").await,
        "search-opt-0"
    );
    assert_eq!(c.ui.attr_of(input, "aria-controls").await, "search-list");
    c.ui.press(input, "ArrowDown").await;
    c.ui.wait_true(
        "document.querySelector('[data-testid=search] input').getAttribute('aria-activedescendant') === 'search-opt-1' && document.getElementById('search-opt-1').getAttribute('aria-selected') === 'true'",
        "ArrowDown moves the active option and the input points at it",
        SECS,
    )
    .await;
}

async fn show_all(c: &Ctx) {
    timed(c, &TYPE.replace("TEXT", "svc00")).await;
    keys(c).await;
    let all: Vec<String> = c
        .ui
        .eval("[...document.querySelectorAll('[data-testid=search] [role=option]')].map(o => o.textContent)")
        .await;
    let i = all
        .iter()
        .position(|t| t.starts_with("Show all 100 services"))
        .unwrap_or_else(|| panic!("a row leads to every match: {all:?}"));
    assert_eq!(
        all.iter().filter(|t| t.starts_with("svc00")).count(),
        5,
        "a few services are shown: {all:?}"
    );
    c.ui.click(&format!("#search-opt-{i} button")).await;
    c.ui.wait_gone("[data-testid=search]", SECS).await;
    c.ui.wait_true(
        "location.pathname.endsWith('/services') && location.search === '?q=svc00'",
        "Show all opens the list with the search",
        SECS,
    )
    .await;
}

async fn version_diff(c: &Ctx, out: &mut Vec<String>) {
    c.world
        .hand_edit(|t| t.replace("    target: 10", "    target: 30"));
    c.ui.wait_text("[data-testid=toasts]", "Config changed", LONG)
        .await;
    c.ui.nav("/services/ssh").await;
    super::edit_mode(c).await;
    c.ui.fill("#f-services-ssh-target", "23").await;
    super::wait_checked(c).await;
    super::review_now(c).await;
    c.ui.wait_for("[data-testid=review]", LONG).await;
    super::apply_now(c, "Applied").await;
    c.ui.nav("/activity").await;
    c.ui.wait_true(
        "document.querySelectorAll('[data-testid=activity] [data-row]').length >= 2",
        "the hand edit shows in Activity with the next change",
        LONG,
    )
    .await;
    let rev =
        c.ui.all_attrs("[data-testid=activity] [data-row]", "data-row")
            .await
            .get(1)
            .cloned()
            .expect("the hand edit");
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
    save(
        out,
        format!(
            "version diff of {CHANGED} changed services: shown in {ms} ms; {rows} rows and {nodes} elements in the DOM"
        ),
    );
    assert_eq!(rows, 400, "a large diff shows its first lines");
    c.ui.click("[data-testid=activity-diff] ~ [data-testid=diff-more] [data-testid=show-more], [data-testid=activity-diff] [data-testid=diff-more] [data-testid=show-more]")
        .await;
    c.ui.wait_true(
        "document.querySelectorAll('[data-testid=activity-diff] [role=row]').length === 800",
        "Show more adds the next lines",
        SECS,
    )
    .await;
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
    save(
        out,
        format!(
            "review of reverting {CHANGED} services: shown in {ms} ms; {paths} service lines, {svgs} svg elements in the service paths, {rows} diff rows, {nodes} elements on the page"
        ),
    );
    assert_eq!(paths, 20, "the first services of a large change are listed");
    assert_eq!(
        c.ui.count("[data-testid=review-paths] [role=group]").await,
        0,
        "a folded service draws no diagram"
    );
    assert_eq!(c.ui.count("[data-testid=review-paths-more]").await, 1);
    c.ui.click("[data-testid=review-path-line]").await;
    c.ui.wait_for(
        "[data-testid=review-paths] details[open] [role=group]",
        SECS,
    )
    .await;
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
        c.ui.assert_no_errors();
    })
}

#[test]
fn search_review_and_diffs_stay_fast_at_scale() {
    run("scale-rest", opts(), false, scale_rest);
}

async fn profiled(c: &Ctx, act: &str) -> String {
    use chromiumoxide::cdp::js_protocol::profiler::{
        EnableParams, SetSamplingIntervalParams, StartParams, StopParams,
    };
    let page = &c.ui.page;
    page.execute(EnableParams::default())
        .await
        .expect("profiler");
    page.execute(SetSamplingIntervalParams::new(100))
        .await
        .expect("interval");
    page.execute(StartParams::default()).await.expect("start");
    timed(c, act).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let p = page
        .execute(StopParams::default())
        .await
        .expect("stop")
        .result
        .profile;
    let mut parent = std::collections::HashMap::new();
    let mut name = std::collections::HashMap::new();
    for n in &p.nodes {
        name.insert(n.id, n.call_frame.function_name.clone());
        for ch in n.children.iter().flatten() {
            parent.insert(*ch, n.id);
        }
    }
    let mut total: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for (s, dt) in p
        .samples
        .unwrap_or_default()
        .iter()
        .zip(p.time_deltas.unwrap_or_default())
    {
        let mut seen = std::collections::HashSet::new();
        let mut at = Some(*s);
        while let Some(id) = at {
            let f = name.get(&id).cloned().unwrap_or_default();
            if seen.insert(f.clone()) {
                *total.entry(f).or_default() += dt as f64 / 1000.0;
            }
            at = parent.get(&id).copied();
        }
    }
    let mut top: Vec<(String, f64)> = total.into_iter().collect();
    top.sort_by(|a, b| b.1.total_cmp(&a.1));
    top.iter()
        .take(60)
        .map(|(f, ms)| format!("{ms:8.1}  {}", f.chars().take(160).collect::<String>()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn scale_form(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.viewport(1440, 900).await;
        c.ui.nav("/overview").await;
        let shown = until(
            c,
            "const run = (location.pathname.match(/^\\/r-[0-9a-f]+/) || [''])[0]; history.pushState(null, '', run + '/services/svc0042'); dispatchEvent(new PopStateEvent('popstate'))",
            "[data-testid=path-diagram]",
        )
        .await
        .round();
        super::edit_mode(c).await;
        let field = "#f-services-svc0042-target";
        c.ui.wait_for(field, SECS).await;
        let mut typed = Vec::new();
        for text in ["2", "20", "200", "2004", "20042", "10042"] {
            typed.push(
                timed(
                    c,
                    &format!("const i = document.querySelector('{field}'); i.value = '{text}'; i.dispatchEvent(new Event('input', {{ bubbles: true }}))"),
                )
                .await
                .round(),
            );
        }
        let top = profiled(
            c,
            &format!("const i = document.querySelector('{field}'); i.value = '20043'; i.dispatchEvent(new Event('input', {{ bubbles: true }}))"),
        )
        .await;
        let text = format!(
            "service page among {SERVICES} services: shown in {shown} ms; typing in its target {typed:?} ms\nwhere one keystroke spends its time (inclusive ms):\n{top}\n"
        );
        println!("{text}");
        let dir = super::sheet::shots_dir().join("scale-form");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("timings.txt"), text);
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_service_form_stays_fast_at_scale() {
    run("scale-form", opts(), false, scale_form);
}
