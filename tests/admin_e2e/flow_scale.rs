use std::time::Duration;

use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, open, run};

const EXTRA: usize = 320;
const SERVICES: usize = EXTRA + 2;
const CLIENTS: usize = 50;
const PORTS: &str = "[data-testid=flow] [data-node^='port:']";
const CLIENT_NODES: &str = "[data-testid=flow] [data-node^='client:']";
const TIP: &str = "[data-testid=flow] [data-testid=tooltip]";
const SEARCH: &str = "#flow-search-box";

fn opts() -> Options {
    Options {
        extra_services: (1..=EXTRA)
            .map(|i| {
                let client = if i % 5 == 0 {
                    "home".to_string()
                } else {
                    format!("box{:02}", i % (CLIENTS - 2) + 1)
                };
                format!(
                    "  app{i:03}:\n    client: {client}\n    target: {}\n    visitors: [laptop]\n",
                    20000 + i
                )
            })
            .collect(),
        extra_clients: (1..=CLIENTS - 2)
            .map(|i| format!("  box{i:02}:\n    secret: {}\n", super::world::secret()))
            .collect(),
        ..Options::default()
    }
}

fn node(key: &str) -> String {
    format!("document.querySelector(\"[data-testid=flow] [data-node='{key}']\")")
}

async fn hover(c: &Ctx, key: &str) {
    let _: bool = c
        .ui
        .eval(&format!("(() => {{ const n = {}; const r = n.getBoundingClientRect(); n.dispatchEvent(new MouseEvent('mousemove', {{ bubbles: true, clientX: r.left + 10, clientY: r.top + 10 }})); return true; }})()", node(key)))
        .await;
    c.ui.wait_for(TIP, SECS).await;
}

async fn nodes_stay(c: &Ctx) {
    let _: bool = c
        .ui
        .eval(&format!(
            "(() => {{ {n}.__kept = 1; {s}.__kept = 1; document.querySelector(\"[data-testid=flow] [data-line='in:app001']\").__kept = 1; return true; }})()",
            n = node("port:app001"),
            s = node("server"),
        ))
        .await;
    let before: String = c.ui.eval(&format!("{}.textContent", node("server"))).await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let kept: bool = c
        .ui
        .eval(&format!(
            "{n}.__kept === 1 && {s}.__kept === 1 && document.querySelector(\"[data-testid=flow] [data-line='in:app001']\").__kept === 1",
            n = node("port:app001"),
            s = node("server"),
        ))
        .await;
    assert!(
        kept,
        "the nodes and lines of the chart survive the live ticks (server text was {before:?})"
    );
    assert_eq!(
        c.ui.count(PORTS).await,
        SERVICES,
        "no node is added by a tick"
    );
}

async fn narrow(c: &mut Ctx) {
    c.ui.fill(SEARCH, "app01").await;
    c.ui.wait_true(
        &format!("document.querySelectorAll(\"{PORTS}\").length === 10"),
        "the search keeps app010 to app019",
        SECS,
    )
    .await;
    c.ui.wait_gone(TIP, SECS).await;
    let count = c.ui.text("[data-testid=flow-count]").await;
    assert_eq!(count.trim(), format!("10 of {SERVICES}"));
    let clients = c.ui.count(CLIENT_NODES).await;
    assert!(
        (1..CLIENTS).contains(&clients),
        "only the clients of the services found are drawn: {clients}"
    );
    c.ui.view_shot("flow-scale-search").await;
    c.ui.viewport(390, 844).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.ui.view_shot("flow-scale-search-phone").await;
    c.ui.fill(SEARCH, "nothing-like-this").await;
    c.ui.wait_for("[data-testid=flow-none]", SECS).await;
    c.ui.view_shot("flow-scale-no-match-phone").await;
    c.ui.viewport(1440, 900).await;
    c.ui.fill(SEARCH, "").await;
    c.ui.wait_true(
        &format!("document.querySelectorAll(\"{PORTS}\").length === {SERVICES}"),
        "clearing the search draws every service again",
        SECS,
    )
    .await;
    assert_eq!(
        c.ui.count(TIP).await,
        0,
        "the old tooltip does not come back"
    );
}

fn flow_scale(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.viewport(1440, 900).await;
        c.ui.click("[data-testid=flow-open]").await;
        c.ui.wait_for(&format!("{PORTS}[data-node='port:app{EXTRA:03}']"), SECS)
            .await;
        assert_eq!(
            c.ui.count(PORTS).await,
            SERVICES,
            "one port node per service"
        );
        assert_eq!(
            c.ui.count("[data-testid=flow] [data-node^='target:']")
                .await,
            SERVICES,
            "one target node per service"
        );
        assert_eq!(
            c.ui.count(CLIENT_NODES).await,
            CLIENTS,
            "one node per client"
        );
        nodes_stay(c).await;
        c.ui.view_shot("flow-scale").await;
        hover(c, "port:app001").await;
        narrow(c).await;
        c.ui.press("[data-testid=flow]", "Escape").await;
        c.ui.wait_gone("[data-testid=flow]", SECS).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_flow_chart_stays_live_and_searchable_with_many_services() {
    run("flow-scale", opts(), false, flow_scale);
}
