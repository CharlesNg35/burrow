use std::sync::OnceLock;
use std::time::Duration;

use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, open, run};

const FLOW: &str = "[data-testid=flow]";
const PORTS: &str = "[data-testid=flow] [data-node^='port:']";
const TIP: &str = "[data-testid=flow] [data-testid=tooltip]";
const SEARCH: &str = "#flow-search-box";

const BOXES: [&str; 3] = ["box1", "box2", "box3"];

fn box_secrets() -> &'static Vec<String> {
    static SECRETS: OnceLock<Vec<String>> = OnceLock::new();
    SECRETS.get_or_init(|| BOXES.iter().map(|_| super::world::secret()).collect())
}

fn opts(n: usize, clients: &[&'static str]) -> Options {
    let mut extra: String = (1..=n)
        .map(|i| {
            let client = clients[i % clients.len()];
            format!(
                "  app{i:04}:\n    client: {client}\n    target: {}\n    visitors: [laptop]\n",
                20000 + i % 30000
            )
        })
        .collect();
    extra.push_str("  zz-down:\n    client: home\n    target: 127.0.0.1:1\n    visitors: [laptop]\n    health_check: { interval_secs: 1, timeout_secs: 1, failures: 1 }\n");
    let extra_clients = BOXES
        .iter()
        .zip(box_secrets())
        .filter(|(b, _)| clients.contains(b))
        .map(|(b, s)| format!("  {b}:\n    secret: {s}\n"))
        .collect();
    Options {
        clients: Some(vec!["home", "office"]),
        extra_services: extra,
        extra_clients,
        ..Options::default()
    }
}

fn start_boxes(c: &mut Ctx) {
    for (b, s) in BOXES.iter().zip(box_secrets()) {
        c.world.secrets.insert(*b, s.clone());
        c.world.start_client(b);
    }
    for b in BOXES {
        c.world.wait_client(b, true);
    }
}

fn node(key: &str) -> String {
    format!("document.querySelector(\"[data-testid=flow] [data-node='{key}']\")")
}

fn sel(key: &str) -> String {
    format!("[data-testid=flow] [data-node='{key}']")
}

async fn open_timed(c: &Ctx, n: usize) -> f64 {
    let ms: f64 = c
        .ui
        .eval(&format!(
            "(async () => {{ const t0 = performance.now(); document.querySelector('[data-testid=flow-open]').click(); while (!document.querySelector(\"{}\")) await new Promise(r => requestAnimationFrame(r)); await new Promise(r => requestAnimationFrame(r)); return performance.now() - t0; }})()",
            sel("bundle:home")
        ))
        .await;
    println!("flow-scale: {n} services: open click to drawn chart in {ms:.0} ms");
    ms
}

async fn nodes_stay(c: &Ctx, keys: &[&str]) {
    let all = keys.iter().map(|k| node(k)).collect::<Vec<_>>();
    let mark = all
        .iter()
        .map(|n| format!("{n}.__kept = 1;"))
        .collect::<String>();
    let _: bool =
        c.ui.eval(&format!("(() => {{ {mark} return true; }})()"))
            .await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let check = all
        .iter()
        .map(|n| format!("({n} && {n}.__kept === 1)"))
        .collect::<Vec<_>>()
        .join(" && ");
    let kept: bool = c.ui.eval(&check).await;
    assert!(kept, "{keys:?} keep their elements across the live ticks");
}

async fn hover(c: &Ctx, key: &str) {
    let _: bool = c
        .ui
        .eval(&format!("(() => {{ const n = {}; const r = n.getBoundingClientRect(); n.dispatchEvent(new MouseEvent('mousemove', {{ bubbles: true, clientX: r.left + 10, clientY: r.top + 10 }})); return true; }})()", node(key)))
        .await;
    c.ui.wait_for(TIP, SECS).await;
}

async fn ports_are(c: &Ctx, n: usize, what: &str) {
    c.ui.wait_true(
        &format!("document.querySelectorAll(\"{PORTS}\").length === {n}"),
        what,
        SECS,
    )
    .await;
}

async fn phone_shot(c: &mut Ctx, step: &str) {
    c.ui.viewport(390, 844).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.ui.view_shot(step).await;
    c.ui.viewport(1440, 900).await;
}

async fn down_stays_apart(c: &Ctx) {
    c.ui.wait_for(&sel("port:zz-down"), Duration::from_secs(40))
        .await;
    let folded: String = c.ui.attr_of(&sel("bundle:home"), "aria-expanded").await;
    assert_eq!(folded, "false", "home is folded while zz-down shows");
    let text: String =
        c.ui.eval(&format!("{}.textContent", node("bundle:home")))
            .await;
    assert!(
        text.contains("1 down"),
        "the folded row counts the down service: {text:?}"
    );
}

async fn search(c: &mut Ctx, total: usize) {
    c.ui.fill(SEARCH, "app001").await;
    ports_are(c, 10, "the search draws app0010 to app0019 one by one").await;
    c.ui.wait_gone(TIP, SECS).await;
    let count = c.ui.text("[data-testid=flow-count]").await;
    assert_eq!(count.trim(), format!("10 of {total}"));
    assert_eq!(
        c.ui.count("[data-testid=flow] [data-node^='bundle:']")
            .await,
        0
    );
}

fn small(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.viewport(1440, 900).await;
        let total = 500 + 3;
        open_timed(c, total).await;
        assert!(c.ui.count(&sel("bundle:office")).await == 1);
        let drawn: String = c.ui.attr_of("[data-testid=flow-svg]", "data-nodes").await;
        let drawn: usize = drawn.parse().expect("data-nodes");
        assert!(
            drawn < 40,
            "500 services fold into a few rows: {drawn} nodes"
        );
        nodes_stay(c, &["bundle:home", "targets:office", "server"]).await;
        c.ui.view_shot("flow-scale-500").await;
        hover(c, "bundle:office").await;
        search(c, total).await;
        c.ui.view_shot("flow-scale-500-search").await;
        phone_shot(c, "flow-scale-500-search-phone").await;
        c.ui.fill(SEARCH, "nothing-like-this").await;
        c.ui.wait_for("[data-testid=flow-none]", SECS).await;
        c.ui.fill(SEARCH, "").await;
        c.ui.wait_for(&sel("bundle:home"), SECS).await;
        assert_eq!(
            c.ui.count(TIP).await,
            0,
            "the old tooltip does not come back"
        );
        let _: bool = c
            .ui
            .eval(&format!("(() => {{ const i = document.querySelector('{SEARCH}'); i.focus(); i.dispatchEvent(new KeyboardEvent('keydown', {{ key: 'Escape', bubbles: true }})); return true; }})()"))
            .await;
        c.ui.wait_gone(FLOW, SECS).await;
        c.ui.assert_no_errors();
    })
}

async fn unfold(c: &mut Ctx) {
    let before: String = c.ui.attr_of("[data-testid=flow-svg]", "data-nodes").await;
    c.ui.click(&sel("bundle:office")).await;
    c.ui.wait_for(
        &format!("{}[aria-expanded=true]", sel("bundle:office")),
        SECS,
    )
    .await;
    let after: String = c.ui.attr_of("[data-testid=flow-svg]", "data-nodes").await;
    let (before, after): (usize, usize) = (before.parse().unwrap(), after.parse().unwrap());
    assert!(
        after > before + 1800,
        "office unfolds its services: {before} -> {after}"
    );
    let drawn = c.ui.count("[data-testid=flow] [data-node]").await;
    assert!(
        drawn < 400,
        "only the rows in view are in the page: {drawn}"
    );
    c.ui.view_shot("flow-scale-5000-unfolded").await;
    c.ui.click(&sel("bundle:office")).await;
    c.ui.wait_for(
        &format!("{}[aria-expanded=false]", sel("bundle:office")),
        SECS,
    )
    .await;
    c.ui.press(&sel("bundle:home"), "Enter").await;
    c.ui.wait_for(&format!("{}[aria-expanded=true]", sel("bundle:home")), SECS)
        .await;
    c.ui.press(&sel("bundle:home"), "Enter").await;
    c.ui.wait_for(
        &format!("{}[aria-expanded=false]", sel("bundle:home")),
        SECS,
    )
    .await;
}

fn large(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        start_boxes(c);
        open(c, "ops").await;
        c.ui.viewport(1440, 900).await;
        let total = 5000 + 3;
        open_timed(c, total).await;
        assert!(c.ui.count(&sel("bundle:office")).await == 1);
        down_stays_apart(c).await;
        nodes_stay(c, &["bundle:home", "port:zz-down", "server"]).await;
        c.ui.view_shot("flow-scale-5000").await;
        phone_shot(c, "flow-scale-5000-phone").await;
        unfold(c).await;
        c.ui.fill(SEARCH, "app0042").await;
        c.ui.wait_for(&sel("port:app0042"), SECS).await;
        c.ui.click("[data-testid=flow-close]").await;
        c.ui.wait_gone(FLOW, SECS).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_flow_chart_folds_500_services_and_stays_live() {
    run(
        "flow-scale-500",
        opts(500, &["home", "office"]),
        false,
        small,
    );
}

#[test]
fn the_flow_chart_stays_quick_with_5000_services() {
    let all = ["home", "office", BOXES[0], BOXES[1], BOXES[2]];
    run("flow-scale-5000", opts(5000, &all), false, large);
}
