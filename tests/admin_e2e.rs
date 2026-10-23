#[path = "admin_e2e/checks.rs"]
mod checks;
#[path = "admin_e2e/edits.rs"]
mod edits;
#[path = "admin_e2e/secrets.rs"]
mod secrets;
#[path = "admin_e2e/sheet.rs"]
mod sheet;
#[path = "admin_e2e/ui.rs"]
mod ui;
#[path = "admin_e2e/world.rs"]
mod world;

use std::future::Future;
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Command;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use futures_util::FutureExt;

use checks::{Baseline, ax_names, ax_problems, compare};
use sheet::Flow;
use ui::Ui;
use world::{BIN, Options, World};

const SECS: Duration = Duration::from_secs(10);

struct Gate {
    chrome: PathBuf,
    chrome_version: String,
    hooks: bool,
}

fn in_ci() -> bool {
    std::env::var("BURROW_E2E_REQUIRE").as_deref() == Ok("1")
}

fn skip(why: &str) -> Option<Gate> {
    assert!(
        !in_ci(),
        "admin e2e is required (BURROW_E2E_REQUIRE=1) but cannot run: {why}"
    );
    println!("skipping the burrow admin end-to-end flow: {why}");
    None
}

fn gate() -> Option<Gate> {
    let info = Command::new(BIN)
        .args(["admin", "--ui-info"])
        .output()
        .expect("burrow admin --ui-info");
    let info = String::from_utf8_lossy(&info.stdout).to_string();
    if info.contains("placeholder") {
        return skip("this burrow has no admin UI built in; run cargo xtask ui first");
    }
    let Some(chrome) = ui::chrome() else {
        panic!(
            "Chrome for Testing {} is missing at {}: run cargo xtask tools to get the pinned Chrome (BURROW_E2E_CHROME or BURROW_E2E_SYSTEM_CHROME=1 use another one)",
            ui::pinned_version(),
            ui::pinned_chrome().display()
        );
    };
    let version = Command::new(&chrome)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let chrome_version = version
        .split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or("unknown")
        .to_string();
    Some(Gate {
        chrome,
        chrome_version,
        hooks: info.contains("test_hooks: true"),
    })
}

static SLOTS: Mutex<usize> = Mutex::new(0);
static FREED: Condvar = Condvar::new();
const MAX_BROWSERS: usize = 1;

struct Slot;

impl Slot {
    fn take() -> Slot {
        let mut n = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        while *n >= MAX_BROWSERS {
            n = FREED.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n += 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut n = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
        *n -= 1;
        FREED.notify_one();
    }
}

pub struct Ctx {
    ui: Ui,
    world: World,
    chrome_version: String,
}

type Body = for<'a> fn(&'a mut Ctx) -> Pin<Box<dyn Future<Output = ()> + 'a>>;

fn run(flow: &str, opts: Options, needs_hooks: bool, body: Body) {
    let Some(g) = gate() else { return };
    if needs_hooks && !g.hooks {
        if in_ci() {
            panic!("{flow} needs a burrow built with --features admin-test-hooks");
        }
        println!("skipping {flow}: build with --features admin-test-hooks");
        return;
    }
    let _slot = Slot::take();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let flow_name = flow.to_string();
    rt.block_on(async move {
        let world = World::new(opts);
        let profile = world.file("chrome-profile");
        let ui = Ui::launch(&g.chrome, Flow::new(&flow_name), &profile).await;
        let mut ctx = Ctx {
            ui,
            world,
            chrome_version: g.chrome_version.clone(),
        };
        let result = std::panic::AssertUnwindSafe(body(&mut ctx))
            .catch_unwind()
            .await;
        let errors = ctx.ui.take_errors();
        match result {
            Ok(()) => {
                ctx.ui.flow.finish(true, "");
                ctx.ui.close().await;
            }
            Err(p) => {
                let msg = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "panic".into());
                let png = tokio::time::timeout(SECS, ctx.ui.png())
                    .await
                    .unwrap_or_default();
                ctx.ui.flow.failure("after-failure.png", &png);
                ctx.ui
                    .flow
                    .failure("console.txt", errors.join("\n").as_bytes());
                ctx.ui
                    .flow
                    .failure("processes.txt", ctx.world.logs().as_bytes());
                ctx.ui.flow.finish(false, &msg);
                ctx.ui.close().await;
                std::panic::resume_unwind(p);
            }
        }
    });
}

async fn open(c: &mut Ctx, visitor: &str) {
    let url = c.world.open_admin(visitor);
    c.ui.goto(&url).await;
    c.ui.wait_for("[data-testid=conn-pill]", SECS).await;
    c.ui.wait_gone("[data-testid=loading]", Duration::from_secs(20))
        .await;
    let now: String = c.ui.eval("location.href").await;
    assert!(
        !now.contains("token"),
        "the token is still in the address bar: {now}"
    );
}

pub async fn wait_checked(c: &Ctx) {
    c.ui.wait_for("[data-testid=check-state]", SECS).await;
    c.ui.wait_for("[data-testid=check-state]:not([data-state=checking])", SECS)
        .await;
}

pub async fn review_now(c: &Ctx) {
    if c.ui
        .count("[data-testid=save-bar] [data-testid=save]")
        .await
        > 0
        && c.ui
            .enabled("[data-testid=save-bar] [data-testid=save]")
            .await
    {
        c.ui.click("[data-testid=save-bar] [data-testid=save]")
            .await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        wait_checked(c).await;
    }
    c.ui.click("[data-testid=review-open]").await;
}

pub async fn edit_mode(c: &Ctx) {
    if c.ui.count("[data-testid=save-bar]").await == 0 {
        c.ui.click("[data-testid=edit]").await;
    }
    c.ui.wait_for("[data-testid=save-bar]", SECS).await;
}

pub async fn open_review(c: &mut Ctx, step: &str) {
    review_now(c).await;
    c.ui.wait_for("[data-testid=review]", SECS).await;
    c.ui.shot(step).await;
}

pub async fn applied(c: &mut Ctx, step: &str) {
    c.ui.wait_for("[data-testid=apply]:not([disabled])", SECS)
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.ui.click("[data-testid=apply]").await;
    c.ui.wait_text(
        "[data-testid=toasts]",
        "Applied rev",
        Duration::from_secs(20),
    )
    .await;
    c.ui.wait_gone("[data-testid=review]", SECS).await;
    c.ui.shot(step).await;
}

async fn ax_clean(c: &Ctx, screen: &str) {
    let tree = c.ui.ax_tree().await;
    let problems = ax_problems(&tree);
    assert!(
        problems.is_empty(),
        "accessibility problems on {screen}: {problems:#?}"
    );
}

async fn baseline(c: &mut Ctx, name: &str) {
    let _: bool = c
        .ui
        .eval("(() => { for (const e of document.querySelectorAll('.font-mono, .tabular-nums, [data-testid=server-line], [data-testid=you], [data-tone] .tabular-nums')) e.style.visibility = 'hidden'; return true; })()")
        .await;
    let png = c.ui.png().await;
    let _: bool = c
        .ui
        .eval("(() => { for (const e of document.querySelectorAll('[style]')) e.style.visibility = ''; return true; })()")
        .await;
    match compare(name, &png, &c.chrome_version) {
        Baseline::Same | Baseline::Written => {}
        Baseline::Skipped(why) => println!("baseline {name}: {why}"),
        Baseline::Differs { ratio, diff } => {
            c.ui.flow
                .failure(&format!("baseline-{name}-diff.png"), &diff);
            c.ui.flow
                .failure(&format!("baseline-{name}-actual.png"), &png);
            panic!(
                "screen {name} differs from its baseline in {:.2}% of pixels",
                ratio * 100.0
            );
        }
    }
}

fn dashboard(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        let echo = echo_server(c.world.ports["echo"]).await;
        open(c, "ops").await;
        c.ui.wait_text("[data-testid=kpi-clients]", "1 away", SECS)
            .await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
            .await;
        c.ui.shot("connected-one-client-online").await;

        c.world.start_client("office");
        c.ui.wait_text(
            "[data-testid=kpi-clients]",
            "all connected",
            Duration::from_secs(5),
        )
        .await;
        c.ui.click("[data-testid=overview-tabs] [data-tab=clients]")
            .await;
        c.ui.wait_for(
            "[data-testid=overview-clients] tr[data-row=office][data-online=true]",
            SECS,
        )
        .await;
        c.ui.shot("office-online-without-reload").await;
        c.ui.click("[data-testid=overview-tabs] [data-tab=services]")
            .await;

        let rate = "[data-testid=overview-services] tr[data-row=web] [data-cell=rate]";
        c.ui.wait_text(rate, "0 bit/s", SECS).await;
        c.ui.shot("before-traffic").await;
        let port = c.world.ports["web"];
        let pump = tokio::spawn(async move {
            for _ in 0..40 {
                if round_trip(port, 256 * 1024).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        c.ui.wait_not_text(rate, "0 bit/s", SECS).await;
        c.ui.wait_for("[data-testid=traffic-chart] path.stroke-accent", SECS)
            .await;
        c.ui.shot("traffic-moves-the-rates").await;
        c.ui.click("[data-testid=flow-open]").await;
        c.ui.wait_for("[data-testid=flow] [data-node='port:web']", SECS)
            .await;
        let _: bool = c
            .ui
            .eval("(() => { const n = document.querySelector(\"[data-testid=flow] [data-node='port:web']\"); const r = n.getBoundingClientRect(); n.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: r.left + 10, clientY: r.top + 10 })); return true; })()")
            .await;
        c.ui.wait_for("[data-testid=flow] .bg-tip", SECS).await;
        let left: String =
            c.ui.eval("document.querySelector('[data-testid=flow] .bg-tip').style.left")
                .await;
        assert!(
            left.ends_with("px"),
            "the flow tooltip is placed without an inline style attribute: {left:?}"
        );
        c.ui.shot("flow-chart").await;
        c.ui.press("[data-testid=flow]", "Escape").await;
        c.ui.wait_gone("[data-testid=flow]", SECS).await;
        c.ui.nav("/services/web").await;
        c.ui.wait_for("[data-testid=live]", SECS).await;
        c.ui.shot("service-live-during-traffic").await;
        pump.abort();
        echo.abort();
        c.ui.assert_no_errors();
    })
}

#[test]
fn e1_dashboard_shows_live_status_and_traffic() {
    run("e1-dashboard", Options::default(), false, dashboard);
}

fn fallback(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "viewer").await;
        c.ui.shot("overview-against-a-newer-server").await;
        c.ui.nav("/services/capped?tab=settings").await;
        let bw = "[data-schema='services/*/bandwidth']";
        let text = c.ui.wait_text(bw, "40mbit", SECS).await;
        assert!(text.contains("Checked by the server"), "{text}");
        assert_eq!(c.ui.count(&format!("{bw} [data-render=fallback]")).await, 1);
        c.ui.shot("bandwidth-shown-by-its-fallback").await;
        let region = "[data-schema='services/*/region'] [data-testid=raw-node]";
        c.ui.wait_text(region, "needs a newer burrow admin", SECS)
            .await;
        c.ui.shot("unknown-node-needs-a-newer-ui").await;
        c.ui.nav("/about").await;
        c.ui.wait_text("[data-testid=about]", "-future", SECS).await;
        c.ui.shot("about-shows-the-newer-schema").await;
        c.ui.assert_no_errors();
    })
}

fn older(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.wait_for("[data-testid=overview-services]", SECS).await;
        let gone = [
            "[data-schema='services/*/bind']",
            "[data-schema='services/*/health_check']",
            "[data-schema='services/*/mode']",
        ];
        c.ui.nav("/services/web?tab=settings").await;
        c.ui.wait_for("[data-schema='services/*/target']", SECS)
            .await;
        for g in gone {
            assert_eq!(c.ui.count(g).await, 0, "{g} is shown without a schema node");
        }
        edit_mode(c).await;
        c.ui.wait_for("#f-services-web-target", SECS).await;
        for g in gone {
            assert_eq!(
                c.ui.count(g).await,
                0,
                "{g} is edited without a schema node"
            );
        }
        c.ui.shot("service-of-an-older-server").await;
        c.ui.nav("/visitors/ops?tab=access").await;
        c.ui.wait_for("[data-schema='visitors/*/admin']", SECS)
            .await;
        assert_eq!(c.ui.count("[data-schema='visitors/*/admin_from']").await, 0);
        c.ui.shot("visitor-of-an-older-server").await;
        c.ui.nav("/services").await;
        c.ui.wait_for("[data-testid=list-row]", SECS).await;
        c.ui.nav("/about").await;
        c.ui.wait_text("[data-testid=about]", "-older", SECS).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn e8_an_older_server_without_fields_the_screens_place_still_renders() {
    let opts = Options {
        server_env: vec![("BURROW_ADMIN_TEST_SCHEMA".into(), "older".into())],
        ..Options::default()
    };
    run("e8-older", opts, true, older);
}

#[test]
fn e8_a_newer_server_field_renders_through_its_fallback() {
    let opts = Options {
        server_env: vec![("BURROW_ADMIN_TEST_SCHEMA".into(), "future".into())],
        extra_services: "  capped:\n    client: home\n    target: 8080\n    visitors: [laptop]\n    bandwidth: 40mbit\n"
            .into(),
        ..Options::default()
    };
    run("e8-fallback", opts, true, fallback);
}

fn private_udp(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/dns").await;
        c.ui.wait_for("[data-testid=path-diagram]", SECS).await;
        let path = c.ui.text("[data-testid=path-diagram]").await.to_lowercase();
        assert!(
            path.contains("visitors") && path.contains("laptop") && path.contains("udp"),
            "a private UDP service starts at its visitors: {path}"
        );
        assert_eq!(c.ui.count("[data-testid=form-problems]").await, 0);
        assert!(
            !c.ui.text("main").await.contains("needs a public port"),
            "a private UDP service with visitors is fine"
        );
        c.ui.shot("private-udp-service").await;
        c.ui.nav("/visitors/laptop").await;
        c.ui.wait_for("[data-testid=may-use]", SECS).await;
        let may = c.ui.text("[data-testid=may-use]").await;
        assert!(
            may.contains("dns") && may.contains("UDP") && may.contains("ssh"),
            "the visitor page lists its UDP services with a tag: {may}"
        );
        let path = c.ui.text("[data-testid=path-diagram]").await;
        assert!(path.contains("1 TCP · 1 UDP"), "{path}");
        c.ui.shot("visitor-with-a-udp-service").await;
        c.ui.click("[role=tablist] [data-tab=connect]").await;
        c.ui.wait_for("[data-testid=connect] [data-testid=snippets]", SECS)
            .await;
        c.ui.click("[data-testid=connect] [data-tab=file]").await;
        c.ui.wait_for("[data-testid=snippet-file]", SECS).await;
        let snippet = c.ui.text("[data-testid=snippet-file]").await;
        assert!(
            snippet.contains("protocol: udp"),
            "the Connect snippet writes a UDP forward: {snippet}"
        );
        c.ui.shot("connect-with-a-udp-forward").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_private_udp_service_with_visitors_reads_right_everywhere() {
    let opts = Options {
        extra_services:
            "  dns:\n    client: home\n    protocol: udp\n    target: 53\n    visitors: [laptop]\n"
                .into(),
        ..Options::default()
    };
    run("private-udp", opts, false, private_udp);
}

fn at_limit(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        c.world.start_client_with("home", "max_connections: 1\n");
        c.world.wait_client("home", true);
        world::wait_until("home answering busy", Duration::from_secs(20), || {
            c.world.status().is_some_and(|s| {
                s["clients"].as_array().is_some_and(|l| {
                    l.iter()
                        .any(|c| c["name"] == "home" && c["at_limit"] == true)
                })
            })
        });
        open(c, "ops").await;
        for (w, h) in [(1440, 900), (420, 900), (320, 800)] {
            c.ui.viewport(w, h).await;
            c.ui.nav("/clients/home").await;
            c.ui.wait_text(
                "[data-testid=limit-tag]",
                "at limit · max_connections in client.yaml",
                SECS,
            )
            .await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let clear: Vec<f64> = c
                .ui
                .eval("(() => { const t = document.querySelector('[data-testid=limit-tag]').getBoundingClientRect(); const overlaps = [...document.querySelectorAll('header [data-testid=edit], header button')].filter(b => { const r = b.getBoundingClientRect(); return r.width > 0 && t.left < r.right && r.left < t.right && t.top < r.bottom && r.top < t.bottom; }).length; return [overlaps, t.right, document.documentElement.clientWidth, t.width, document.querySelector('[data-testid=limit-tag]').scrollWidth]; })()")
                .await;
            assert_eq!(
                clear[0], 0.0,
                "at {w} px the limit tag runs under a button: {clear:?}"
            );
            assert!(
                clear[1] <= clear[2],
                "at {w} px the limit tag runs off the screen: {clear:?}"
            );
            assert!(
                clear[4] <= clear[3] + 1.0,
                "at {w} px the limit tag text is cut off: {clear:?}"
            );
            c.ui.shot(&format!("at-limit-{w}")).await;
        }
        c.ui.viewport(1440, 900).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_client_at_its_limit_says_which_limit_without_overlapping_the_buttons() {
    let opts = Options {
        clients: Some(Vec::new()),
        extra_services: format!(
            "  dns1: {{ client: home, protocol: udp, bind: 127.0.0.1:{}, target: 53 }}\n  dns2: {{ client: home, protocol: udp, bind: 127.0.0.1:{}, target: 54 }}\n",
            world::free_port(),
            world::free_port()
        ),
        ..Options::default()
    };
    run("at-limit", opts, false, at_limit);
}

fn many_issues(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.wait_for("[data-testid=attention] li:nth-child(14)", SECS)
            .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let heights: Vec<f64> = c
            .ui
            .eval("[document.querySelector('[data-testid=traffic]').getBoundingClientRect().height, document.querySelector('[data-testid=attention-card]').getBoundingClientRect().height]")
            .await;
        assert!(
            (heights[0] - heights[1]).abs() < 1.0,
            "the attention card is exactly as tall as the traffic card: {heights:?}"
        );
        let scrolls: bool = c
            .ui
            .eval("(() => { const l = document.querySelector('[data-testid=attention] ul'); return l.scrollHeight > l.clientHeight + 1; })()")
            .await;
        assert!(scrolls, "the attention rows scroll inside the card");
        c.ui.wait_for(
            "[data-testid=attention] [data-testid=fade]:not(.hidden)",
            SECS,
        )
        .await;
        assert_eq!(
            c.ui.count("[data-testid=attention] [data-testid=more]")
                .await,
            0,
            "no +N more chip: the card scrolls"
        );
        let first: String = c
            .ui
            .eval("document.querySelector('[data-testid=attention] li').getAttribute('data-tone')")
            .await;
        assert_eq!(first, "danger", "danger rows come first");
        c.ui.shot("many-issues-scroll-inside-the-card").await;
        c.ui.click("[data-testid=flow-open]").await;
        c.ui.wait_for("[data-testid=flow]", SECS).await;
        let _: bool = c
            .ui
            .eval("(() => { document.querySelector('[data-testid=flow] input[value=problems]').click(); return true; })()")
            .await;
        c.ui.shot("flow-problems").await;
        c.ui.press("[data-testid=flow]", "Escape").await;
        c.ui.wait_gone("[data-testid=flow]", SECS).await;
        let _: bool = c
            .ui
            .eval("(() => { document.querySelector('[data-testid=attention] ul').scrollTop = 120; return true; })()")
            .await;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let kept: f64 =
            c.ui.eval("document.querySelector('[data-testid=attention] ul').scrollTop")
                .await;
        assert!(
            kept >= 119.0,
            "live updates keep the scroll position: {kept}"
        );
        c.ui.viewport(1100, 900).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let rows: usize = c
            .ui
            .eval("[...document.querySelectorAll('[data-testid=attention] li')].filter(e => e.offsetParent !== null).length")
            .await;
        let all = c.ui.count("[data-testid=attention] li").await;
        assert!(
            rows >= 14 && rows == all,
            "below 1200 px every row is there: {rows} of {all}"
        );
        let scrolls: bool = c
            .ui
            .eval("(() => { const l = document.querySelector('[data-testid=attention] ul'); return l.scrollHeight > l.clientHeight + 1; })()")
            .await;
        assert!(scrolls, "below 1200 px the rows scroll in a capped list");
        c.ui.shot("many-issues-below-1200").await;
        c.ui.viewport(1440, 900).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn many_issues_scroll_inside_a_card_as_tall_as_traffic() {
    let (mut extra, mut clients) = (String::new(), String::new());
    for i in 0..14 {
        clients.push_str(&format!(
            "  spare{i:02}:\n    secret: {}\n",
            world::secret()
        ));
        extra.push_str(&format!(
            "  down{i:02}:\n    clients: [office, spare{i:02}]\n    target: {}\n    visitors: [laptop]\n",
            9100 + i
        ));
    }
    let opts = Options {
        extra_services: extra,
        extra_clients: clients,
        ..Options::default()
    };
    run("many-issues", opts, false, many_issues);
}

fn list_scrolls(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web?tab=settings").await;
        c.ui.wait_for("[data-testid=entity-list] [data-row=svc29]", SECS)
            .await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        let geo: Vec<f64> = c
            .ui
            .eval("(() => { const a = document.querySelector('[data-testid=entity-list]').getBoundingClientRect(); const h = document.querySelector('[data-testid=header]').getBoundingClientRect(); return [a.top, a.height, h.height, window.innerHeight]; })()")
            .await;
        assert!(
            (geo[0] - geo[2]).abs() < 1.5,
            "the list pane starts under the header: {geo:?}"
        );
        assert!(
            (geo[1] - (geo[3] - geo[2])).abs() < 1.5,
            "the list pane is the viewport minus the header: {geo:?}"
        );
        let head_before: f64 = c
            .ui
            .eval("document.querySelector('[data-testid=list-head]').getBoundingClientRect().top")
            .await;
        let rows_scrolled: f64 = c
            .ui
            .eval("(() => { const l = document.querySelector('[data-testid=entity-list] [data-testid=list]'); l.scrollTop = 300; return l.scrollTop; })()")
            .await;
        assert!(rows_scrolled > 0.0, "the rows scroll inside the pane");
        let head_after: f64 = c
            .ui
            .eval("document.querySelector('[data-testid=list-head]').getBoundingClientRect().top")
            .await;
        assert!(
            (head_before - head_after).abs() < 0.5,
            "scrolling the rows leaves the head in place"
        );
        c.ui.shot("rows-scrolled-head-fixed").await;
        let page: Vec<f64> = c
            .ui
            .eval("(() => { window.scrollTo(0, 600); const top = document.querySelector('[data-testid=entity-list]').getBoundingClientRect().top; return [document.scrollingElement.scrollTop, top]; })()")
            .await;
        assert!(page[0] > 100.0, "the detail scrolls the document: {page:?}");
        assert!(
            (page[1] - geo[0]).abs() < 1.5,
            "the list pane stays put while the page scrolls: {page:?}"
        );
        let inner: usize = c
            .ui
            .eval("[...document.querySelectorAll('#detail, #detail *')].filter(e => { const o = getComputedStyle(e).overflowY; return (o === 'auto' || o === 'scroll') && e.scrollHeight > e.clientHeight + 1; }).length")
            .await;
        assert_eq!(inner, 0, "the detail has no scroll container of its own");
        c.ui.shot("detail-scrolls-with-the-page").await;
        let _: bool =
            c.ui.eval("(() => { window.scrollTo(0, 0); return true; })()")
                .await;
        c.ui.nav("/services/svc29").await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        let visible: bool = c
            .ui
            .eval("(() => { const l = document.querySelector('[data-testid=entity-list] [data-testid=list]').getBoundingClientRect(); const r = document.querySelector('[data-row=svc29]').getBoundingClientRect(); return r.top >= l.top - 1 && r.bottom <= l.bottom + 1; })()")
            .await;
        assert!(visible, "the selected row scrolls into view");
        c.ui.viewport(390, 844).await;
        c.ui.nav("/services").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let stacked: f64 = c
            .ui
            .eval("(() => { window.scrollTo(0, 400); return document.scrollingElement.scrollTop; })()")
            .await;
        assert!(stacked > 0.0, "on a phone the list scrolls with the page");
        let switcher = c.ui.text("[data-testid=server-switcher]").await;
        assert!(
            !switcher.contains(" up "),
            "the header shows only the server name on a phone: {switcher}"
        );
        c.ui.viewport(1440, 900).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_list_pane_stays_put_while_the_detail_scrolls() {
    let mut extra = String::new();
    for i in 0..30 {
        extra.push_str(&format!(
            "  svc{i:02}:\n    client: home\n    target: {}\n    visitors: [laptop]\n",
            9200 + i
        ));
    }
    let opts = Options {
        extra_services: extra,
        ..Options::default()
    };
    run("list-scrolls", opts, false, list_scrolls);
}

const MAIN_SCREENS: &[(&str, &str)] = &[
    ("/", "overview"),
    ("/services", "services"),
    ("/services/web", "service-detail"),
    ("/clients", "clients"),
    ("/clients/home", "client-detail"),
    ("/visitors", "visitors"),
    ("/visitors/ops", "visitor-detail"),
    ("/settings", "settings"),
    ("/activity", "activity"),
    ("/about", "about"),
];

fn screens(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "ops").await;
        for (path, name) in MAIN_SCREENS {
            c.ui.nav(path).await;
            c.ui.wait_gone("[aria-busy=true]", SECS).await;
            c.ui.shot(&format!("{name}-desktop")).await;
            ax_clean(c, name).await;
            baseline(c, &format!("{name}-desktop")).await;
        }
        let mut links: Vec<String> = Vec::new();
        for (path, _) in MAIN_SCREENS {
            c.ui.nav(path).await;
            c.ui.wait_gone("[aria-busy=true]", SECS).await;
            let found: Vec<String> = c
                .ui
                .eval("[...document.querySelectorAll('a[href]')].map(a => a.getAttribute('href'))")
                .await;
            for l in found {
                if !links.contains(&l) {
                    links.push(l);
                }
            }
        }
        assert!(links.len() > 10, "links found: {links:?}");
        for l in &links {
            assert!(
                l.starts_with('/') || l.starts_with('#'),
                "every link stays inside burrow admin: {l}"
            );
            if l.starts_with('#') {
                continue;
            }
            c.ui.nav(l).await;
            c.ui.wait_gone("[aria-busy=true]", SECS).await;
            assert_eq!(
                c.ui.count("[data-testid=not-found], [data-testid=detail-missing]")
                    .await,
                0,
                "{l} opens a real page"
            );
        }
        c.ui.assert_no_errors();
        c.ui.nav("/activity").await;
        c.ui.wait_for("[data-testid=activity-empty]", SECS).await;
        c.ui.shot("activity-empty-state").await;
        c.ui.nav("/services/nope").await;
        c.ui.wait_for("[data-testid=detail-missing]", SECS).await;
        c.ui.shot("missing-service").await;
        c.ui.nav("/nowhere/at/all").await;
        c.ui.wait_for("[data-testid=not-found]", SECS).await;
        c.ui.shot("page-not-found").await;

        c.ui.dark(true).await;
        for (path, name) in MAIN_SCREENS {
            c.ui.nav(path).await;
            c.ui.wait_gone("[aria-busy=true]", SECS).await;
            c.ui.shot(&format!("{name}-dark")).await;
        }
        c.ui.click("[data-testid=avatar]").await;
        c.ui.click("[data-theme-choice=light]").await;
        let html_class: String = c.ui.eval("document.documentElement.className").await;
        assert!(
            html_class.contains("light"),
            "the manual light theme overrides the system: {html_class}"
        );
        c.ui.nav("/").await;
        c.ui.shot("manual-light-theme-over-dark-system").await;
        c.ui.click("[data-testid=avatar]").await;
        c.ui.click("[data-theme-choice=system]").await;
        c.ui.dark(false).await;

        c.ui.viewport(390, 844).await;
        for (path, name) in MAIN_SCREENS
            .iter()
            .filter(|(_, n)| !n.ends_with("detail") || *n == "service-detail")
        {
            c.ui.nav(path).await;
            c.ui.wait_gone("[aria-busy=true]", SECS).await;
            c.ui.shot(&format!("{name}-narrow")).await;
            let overflow: bool = c
                .ui
                .eval("document.documentElement.scrollWidth > document.documentElement.clientWidth + 1")
                .await;
            assert!(!overflow, "{name} scrolls sideways at 390 px");
        }
        c.ui.nav("/services/web").await;
        c.ui.click("[data-testid=edit]").await;
        c.ui.wait_for("[data-testid=save-bar]", SECS).await;
        c.ui.shot("service-form-narrow").await;
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.viewport(1100, 900).await;
        c.ui.nav("/").await;
        c.ui.shot("overview-1100").await;
        c.ui.viewport(1920, 1080).await;
        c.ui.shot("overview-1920").await;
        c.ui.viewport(1440, 900).await;
        c.ui.click("[data-testid=search-open]").await;
        c.ui.wait_for("[data-testid=search]", SECS).await;
        c.ui.fill("[data-testid=search] input", "we").await;
        c.ui.wait_text("[data-testid=search]", "web", SECS).await;
        c.ui.shot("search-open").await;
        c.ui.press("[data-testid=search]", "Escape").await;
        c.ui.wait_gone("[data-testid=search]", SECS).await;

        let here: String = c.ui.eval("location.origin + '/'").await;
        c.ui.goto(&here).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
            .await;
        let mut order = Vec::new();
        for _ in 0..4 {
            c.ui.press("body", "Tab").await;
            let label: String = c
                .ui
                .eval("(document.activeElement && (document.activeElement.innerText || document.activeElement.getAttribute('aria-label') || '')).trim()")
                .await;
            order.push(label);
        }
        assert_eq!(order[0], "Skip to content", "focus order {order:?}");
        let names = ax_names(&c.ui.ax_tree().await, "link");
        assert!(names.iter().any(|n| n.contains("Overview")), "{names:?}");
        c.ui.assert_no_errors();
    })
}

#[test]
fn every_screen_renders_in_light_dark_and_narrow_and_is_accessible() {
    run("screens", Options::default(), false, screens);
}

fn no_token(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        let url = c.world.open_admin("ops");
        let bare = url.split('#').next().unwrap_or_default().to_string();
        c.ui.goto(&bare).await;
        c.ui.wait_for("[data-testid=no-token]", SECS).await;
        c.ui.shot("opened-without-the-link").await;
        ax_clean(c, "no-token").await;
        c.ui.goto(&url).await;
        c.ui.wait_gone("[data-testid=loading]", Duration::from_secs(20))
            .await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
            .await;
        c.ui.shot("then-with-the-link").await;
        c.ui.goto(&bare).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
            .await;
        c.ui.shot("reload-keeps-the-session").await;
        c.ui.take_errors();
    })
}

#[test]
fn the_page_needs_the_link_from_the_terminal() {
    run("no-token", Options::default(), false, no_token);
}

async fn header_items(c: &Ctx) -> Vec<String> {
    c.ui.eval(
        "Array.from(document.querySelector('[data-testid=header-right]').children).filter(e => { const r = e.getBoundingClientRect(); return r.width > 2 && r.height > 2; }).map(e => e.getAttribute('data-testid') || e.querySelector('[data-testid]')?.getAttribute('data-testid') || e.tagName)",
    )
    .await
}

pub async fn popover_behaves(c: &Ctx, trigger: &str, panel: &str) {
    let t = format!("#{trigger}");
    let p = format!("#{panel}");
    c.ui.click(&t).await;
    c.ui.wait_for(&p, SECS).await;
    assert_eq!(c.ui.attr_of(&t, "aria-expanded").await, "true", "{trigger}");
    assert_eq!(c.ui.attr_of(&t, "aria-controls").await, panel, "{trigger}");
    let started = std::time::Instant::now();
    loop {
        let inside: bool = c
            .ui
            .eval(&format!(
                "(() => {{ const p = document.getElementById('{panel}'); return !!p && p.contains(document.activeElement); }})()"
            ))
            .await;
        if inside {
            break;
        }
        assert!(started.elapsed() < SECS, "focus moves into {panel}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _: bool = c
        .ui
        .eval("(() => { document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); return true; })()")
        .await;
    c.ui.wait_gone(&p, SECS).await;
    let back: String =
        c.ui.eval("document.activeElement ? document.activeElement.id : ''")
            .await;
    assert_eq!(back, trigger, "Escape returns focus to the trigger");
    assert_eq!(
        c.ui.attr_of(&t, "aria-expanded").await,
        "false",
        "{trigger}"
    );
    c.ui.click(&t).await;
    c.ui.wait_for(&p, SECS).await;
    let _: bool = c
        .ui
        .eval("(() => { document.querySelector('main, #detail, body').dispatchEvent(new PointerEvent('pointerdown', { bubbles: true })); return true; })()")
        .await;
    c.ui.wait_gone(&p, SECS).await;
}

async fn theme_now(c: &Ctx) -> (String, String) {
    let t =
        c.ui.attr_of("[data-testid=theme-toggle]", "data-theme")
            .await;
    let class: String = c.ui.eval("document.documentElement.className").await;
    (t, class)
}

fn calm_header(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        let url = c.world.open_admin("ops");
        c.ui.goto(&url).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected to", SECS)
            .await;
        let items = header_items(c).await;
        assert_eq!(
            items,
            ["search-open", "theme-toggle", "avatar"],
            "the normal header has search, theme and the avatar only"
        );
        c.ui.shot("normal").await;

        assert_eq!(theme_now(c).await.0, "system");
        c.ui.click("[data-testid=theme-toggle]").await;
        let (t, class) = theme_now(c).await;
        assert!(t == "light" && class.contains("light"), "{t} {class}");
        c.ui.click("[data-testid=theme-toggle]").await;
        let (t, class) = theme_now(c).await;
        assert!(t == "dark" && class.contains("dark"), "{t} {class}");
        let bare = url.split('#').next().unwrap_or_default().to_string();
        c.ui.goto(&bare).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected to", SECS)
            .await;
        let (t, class) = theme_now(c).await;
        assert!(
            t == "dark" && class.contains("dark"),
            "the theme survives a reload: {t} {class}"
        );
        c.ui.shot("dark-after-reload").await;
        c.ui.click("[data-testid=theme-toggle]").await;
        let (t, class) = theme_now(c).await;
        assert!(
            t == "system" && !class.contains("dark") && !class.contains("light"),
            "{t} {class}"
        );

        popover_behaves(c, "avatar", "avatar-menu").await;
        popover_behaves(c, "server-switcher", "server-menu").await;
        c.ui.click("[data-testid=avatar]").await;
        c.ui.wait_text("[data-testid=avatar-who]", "ops · admin write", SECS)
            .await;
        c.ui.wait_text("[data-testid=avatar-conn]", "Connected to", SECS)
            .await;
        c.ui.wait_for("[data-testid=avatar-about]", SECS).await;
        c.ui.shot("avatar-menu").await;
        c.ui.click("[data-testid=avatar]").await;

        c.ui.nav("/services/web").await;
        c.ui.wait_for("#entity-more", SECS).await;
        popover_behaves(c, "entity-more", "entity-more-menu").await;
        edit_mode(c).await;
        c.ui.fill("#f-services-web-target", "4321").await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        c.ui.wait_for("[data-testid=review-open]", SECS).await;
        assert_eq!(
            header_items(c).await,
            ["search-open", "theme-toggle", "review-open", "avatar"],
            "Review joins the header only with pending changes"
        );

        c.world.stop("server");
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Reconnecting in",
            Duration::from_secs(15),
        )
        .await;
        assert_eq!(
            header_items(c).await,
            [
                "search-open",
                "theme-toggle",
                "conn-pill",
                "review-open",
                "avatar"
            ],
            "the reconnecting text shows only while reconnecting"
        );
        popover_behaves(c, "conn-pill", "retry-popover").await;
        c.ui.click("[data-testid=conn-pill]").await;
        c.ui.wait_for("[data-testid=retry-now]", SECS).await;
        c.ui.shot("reconnecting").await;
        c.world
            .start("server", &["server", "-c", "server.yaml"], &[]);
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Connected to",
            Duration::from_secs(30),
        )
        .await;
        assert_eq!(
            header_items(c).await,
            ["search-open", "theme-toggle", "review-open", "avatar"],
            "the reconnecting text goes away once connected"
        );
        c.ui.take_errors();
    })
}

#[test]
fn the_header_is_calm_in_every_state() {
    run("header", Options::default(), false, calm_header);
}

fn read_only(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "viewer").await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
            .await;
        c.ui.wait_text("[data-testid=you-level]", "viewer (read)", SECS)
            .await;
        c.ui.wait_for("[data-testid=read-only-tag]", SECS).await;
        assert_eq!(
            c.ui.count("[data-testid=banners] [data-testid^=banner]")
                .await,
            0,
            "read-only shows no banner"
        );
        assert_eq!(
            header_items(c).await,
            ["search-open", "theme-toggle", "read-only-tag", "avatar"],
            "the read-only tag takes the Review button's place"
        );
        popover_behaves(c, "read-only-tag", "read-only-why").await;
        c.ui.click("[data-testid=read-only-tag]").await;
        c.ui.wait_text("[data-testid=read-only-why]", "has read admin", SECS)
            .await;
        c.ui.shot("read-only-overview").await;
        c.ui.click("[data-testid=read-only-tag]").await;
        c.ui.wait_gone("[data-testid=read-only-why]", SECS).await;
        c.ui.nav("/settings").await;
        c.ui.shot("read-only-settings").await;
        ax_clean(c, "read-only").await;

        #[cfg(unix)]
        {
            let file = c.world.file("server.yaml");
            let mut perms = std::fs::metadata(&file).expect("server.yaml").permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o444);
            std::fs::set_permissions(&file, perms.clone()).expect("make server.yaml read-only");
            let url = c.world.open_admin("ops");
            c.ui.goto("about:blank").await;
            c.ui.goto(&url).await;
            c.ui.wait_for("[data-testid=read-only-tag]", SECS).await;
            c.ui.wait_text("[data-testid=conn-pill]", "Connected", SECS)
                .await;
            assert_eq!(
                c.ui.count("[data-testid=banners] [data-testid^=banner]")
                    .await,
                0,
                "a config the server cannot write shows no banner"
            );
            c.ui.click("[data-testid=read-only-tag]").await;
            c.ui.wait_text("[data-testid=read-only-why]", "cannot write", SECS)
                .await;
            c.ui.wait_text("[data-testid=read-only-why]", "write access to", SECS)
                .await;
            c.ui.shot("not-writable-reason").await;
            std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o600);
            std::fs::set_permissions(&file, perms).expect("make server.yaml writable again");
        }
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_read_only_admin_sees_everything_and_is_told_why_it_cannot_change_it() {
    run("read-only", Options::default(), false, read_only);
}

fn leaves(node: &serde_json::Value, path: Vec<String>, out: &mut Vec<String>) {
    let sem = &node["sem"];
    match sem["t"].as_str().unwrap_or_default() {
        "optional" => leaves(&sem["inner"], path, out),
        "object" => {
            for f in sem["fields"].as_array().into_iter().flatten() {
                let mut p = path.clone();
                p.push(f["key"].as_str().unwrap_or_default().to_string());
                leaves(&f["node"], p, out);
            }
        }
        "map" if path.len() == 1 => {
            let mut p = path.clone();
            p.push("*".into());
            leaves(&sem["value"], p, out);
        }
        _ => out.push(path.join("/")),
    }
}

fn schema_walk(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../crates/burrow-admin-proto/tests/fixtures/v0.1/schema.json"
        ))
        .expect("the frozen schema");
        let mut wanted = Vec::new();
        leaves(&schema["root"], Vec::new(), &mut wanted);
        open(c, "ops").await;
        let mut seen: Vec<String> = Vec::new();
        let mut unset: Vec<String> = Vec::new();
        for (path, tabs) in [
            ("/settings", &[][..]),
            ("/clients/office", &["settings", "secrets"][..]),
            ("/visitors/ops", &["access", "secrets"][..]),
            ("/visitors/laptop", &["access"][..]),
            ("/services/web", &["settings"][..]),
            ("/services/game", &["settings"][..]),
            ("/services/dns", &["settings"][..]),
        ] {
            c.ui.nav(path).await;
            let steps: Vec<Option<&str>> = if tabs.is_empty() {
                vec![None]
            } else {
                tabs.iter().map(|t| Some(*t)).collect()
            };
            for tab in steps {
                if let Some(t) = tab {
                    c.ui.click(&format!("[role=tablist] [data-tab={t}]")).await;
                    c.ui.wait_for(
                        &format!("[role=tablist] [data-tab={t}][aria-selected=true]"),
                        SECS,
                    )
                    .await;
                }
                c.ui.wait_for("[data-schema]", SECS).await;
                c.ui.shot(&format!(
                    "walk-{}{}",
                    path.trim_start_matches('/').replace('/', "-"),
                    tab.map(|t| format!("-{t}")).unwrap_or_default()
                ))
                .await;
                seen.extend(c.ui.all_attrs("[data-schema]", "data-schema").await);
                unset.extend(
                    c.ui.all_attrs("[data-schema]:has([data-testid=not-set])", "data-schema")
                        .await,
                );
            }
        }
        let missing: Vec<&String> = wanted
            .iter()
            .filter(|w| !seen.contains(w))
            .filter(|w| !unset.iter().any(|u| w.starts_with(&format!("{u}/"))))
            .collect();
        assert!(
            missing.is_empty(),
            "these config fields are shown nowhere: {missing:#?}"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn every_server_config_field_is_shown_somewhere() {
    let opts = Options {
        clients: Some(vec!["home", "office"]),
        extra_services: format!(
            "  game:\n    clients: [home, office]\n    mode: balance\n    health_check: true\n    bind: 127.0.0.1:{p}\n    target: 7000\n    max_connections: 9\n    allow_from: [127.0.0.1/32]\n    deny_from: [10.0.0.0/8]\n    proxy_protocol: v1\n    bandwidth: 5MB\n  dns:\n    client: office\n    protocol: udp\n    bind: 127.0.0.1:{q}\n    target: 53\n",
            p = world::free_port(),
            q = world::free_port()
        ),
        ..Options::default()
    };
    run("schema-walk", opts, false, schema_walk);
}

#[test]
fn local_web_security_holds_against_other_pages_and_processes() {
    if gate().is_none() {
        return;
    }
    let mut w = World::new(Options::default());
    let url = w.open_admin("ops");
    let port: u16 = url
        .trim_start_matches("http://127.0.0.1:")
        .split('/')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("a port in the url");
    let get = |head: String| -> String {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        s.write_all(head.as_bytes()).expect("write");
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).to_string()
    };
    let good = format!("127.0.0.1:{port}");
    let run = url
        .split('#')
        .next()
        .unwrap_or_default()
        .trim_start_matches(&format!("http://{good}"))
        .trim_end_matches('/')
        .to_string();
    assert!(run.starts_with("/r-") && run.len() == 35, "{url}");
    assert!(get("GET / HTTP/1.1\r\nHost: evil.example\r\n\r\n".into()).starts_with("HTTP/1.1 403"));
    assert!(
        get(format!(
            "GET / HTTP/1.1\r\nHost: evil.example:{port}\r\n\r\n"
        ))
        .starts_with("HTTP/1.1 403")
    );
    assert!(
        get(format!(
            "GET / HTTP/1.1\r\nHost: {good}\r\nSec-Fetch-Site: cross-site\r\n\r\n"
        ))
        .starts_with("HTTP/1.1 403")
    );
    let page = get(format!("GET {run}/ HTTP/1.1\r\nHost: {good}\r\n\r\n"));
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    let root = get(format!("GET / HTTP/1.1\r\nHost: {good}\r\n\r\n"));
    assert!(root.starts_with("HTTP/1.1 302"), "{root}");
    assert!(page.contains("Content-Security-Policy: default-src 'none'"));
    assert!(page.contains("script-src 'self' 'wasm-unsafe-eval'"));
    assert!(!page.to_ascii_lowercase().contains("set-cookie"));
    let upgrade_at = |path: &str, origin: &str| {
        format!(
            "GET {path} HTTP/1.1\r\nHost: {good}\r\nOrigin: {origin}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
    };
    let upgrade = |origin: &str| upgrade_at(&format!("{run}/api"), origin);
    assert!(
        get(upgrade_at("/api", &format!("http://{good}"))).starts_with("HTTP/1.1 404"),
        "the api answers only under this run's prefix"
    );
    assert!(get(upgrade("http://evil.example")).starts_with("HTTP/1.1 403"));
    let started = std::time::Instant::now();
    let silent = get(upgrade(&format!("http://{good}")));
    assert!(silent.starts_with("HTTP/1.1 101"), "{silent}");
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "a socket without the token is closed in about 5 s"
    );
    w.stop("admin-ops");
}

struct Impostor {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Impostor {
    fn start(port: u16, app_js: String) -> Impostor {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("the impostor port");
        listener.set_nonblocking(true).expect("nonblocking");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let page = format!(
            "<!doctype html><html><head><script type=\"module\" src=\"{app_js}\"></script><script>document.documentElement.setAttribute('data-pwned', 'page'); const t = sessionStorage.getItem('burrow-admin-token') || (location.hash.match(/token=([0-9a-f]+)/) || [])[1] || ''; const ws = new WebSocket('ws://' + location.host + '/api'); ws.onopen = () => ws.send(JSON.stringify({{auth: t || ''}})); ws.onmessage = () => document.documentElement.setAttribute('data-api', 'ok'); ws.onerror = () => {{ if (!document.documentElement.getAttribute('data-api')) document.documentElement.setAttribute('data-api', 'refused'); }}; ws.onclose = ws.onerror; setTimeout(() => ws.onerror(), 4000);</script></head><body>PWNED impostor</body></html>"
        );
        let trojan = "export default function init() { document.documentElement.setAttribute('data-pwned', 'script'); document.body.textContent = 'PWNED ' + location.hash; }\n".to_string();
        let thread = std::thread::spawn(move || {
            let mut served = Vec::new();
            while !flag.load(std::sync::atomic::Ordering::SeqCst) {
                let Ok((mut s, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                };
                let (page, trojan) = (page.clone(), trojan.clone());
                served.push(std::thread::spawn(move || {
                    let _ = s.set_nonblocking(false);
                    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let head = String::from_utf8_lossy(&buf[..n]);
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let (body, kind) = if path.ends_with(".js") {
                        (trojan, "text/javascript")
                    } else if path.contains('.') {
                        (String::new(), "application/octet-stream")
                    } else {
                        (page, "text/html")
                    };
                    let _ = write!(
                        s,
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nCache-Control: public, max-age=31536000, immutable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }));
            }
            drop(listener);
            for t in served {
                let _ = t.join();
            }
        });
        Impostor {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Impostor {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn impostor(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        let info = Command::new(world::BIN)
            .args(["admin", "--ui-info"])
            .output()
            .expect("burrow admin --ui-info");
        let app_js = String::from_utf8_lossy(&info.stdout)
            .split_whitespace()
            .find(|w| w.starts_with("/app-") && w.ends_with(".js"))
            .map(str::to_string)
            .unwrap_or_else(|| "/app.js".into());
        let port = world::free_port();
        let fake = Impostor::start(port, app_js.clone());
        c.ui.goto("about:blank").await;
        let _: bool = c.ui.eval("true").await;
        for path in ["/services/web", "/clients", app_js.as_str(), "/"] {
            c.ui.goto(&format!("http://127.0.0.1:{port}{path}")).await;
        }
        c.ui.wait_for("[data-pwned]", SECS).await;
        c.ui.shot("the-impostor-before-burrow-admin").await;
        c.ui.goto("about:blank").await;
        drop(fake);
        let gap = std::time::Instant::now();
        loop {
            let addr = ("127.0.0.1", port)
                .to_socket_addrs()
                .unwrap()
                .next()
                .unwrap();
            if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
                break;
            }
            assert!(
                gap.elapsed() < Duration::from_secs(10),
                "the impostor still holds port {port}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let url = c.world.open_admin_on("ops", port);
        let path = url
            .trim_start_matches(&format!("http://127.0.0.1:{port}"))
            .split('#')
            .next()
            .unwrap_or("/")
            .to_string();
        let started = std::time::Instant::now();
        loop {
            use std::io::{Read, Write};
            let real = std::net::TcpStream::connect(("127.0.0.1", port))
                .ok()
                .and_then(|mut sock| {
                    write!(
                        sock,
                        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
                    )
                    .ok()?;
                    let mut out = String::new();
                    let _ = sock.read_to_string(&mut out);
                    Some(out)
                })
                .is_some_and(|g| g.contains(&format!("src=\"{path}")));
            if real {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "burrow admin did not take over {port}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let token = url.split("#token=").nth(1).unwrap_or_default().to_string();
        c.ui.goto(&format!("http://127.0.0.1:{port}/#token={token}"))
            .await;
        c.ui.wait_for("[data-api]", SECS).await;
        c.ui.shot("plain-root-from-the-poisoned-cache").await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let api: String =
            c.ui.eval("document.documentElement.getAttribute('data-api') || ''")
                .await;
        assert_ne!(
            api, "ok",
            "a page from the poisoned cache reached the admin API with the launch token"
        );
        let pwned: bool =
            c.ui.eval("document.documentElement.getAttribute('data-pwned') != null")
                .await;
        let redirected: String = c.ui.eval("location.pathname").await;
        assert!(
            pwned || redirected.starts_with("/r-"),
            "the plain root neither served the cached impostor nor redirected to the run: {redirected}"
        );
        c.ui.take_errors();
        c.ui.take_errors();
    })
}

#[test]
fn an_impostor_that_held_the_port_earlier_never_runs_in_the_real_ui() {
    run("impostor", Options::default(), false, impostor);
}

#[test]
fn a_visitor_without_admin_rights_is_refused_in_the_terminal() {
    let w = World::new(Options {
        clients: Some(Vec::new()),
        ..Options::default()
    });
    let file = w.visitor_file("plain");
    let out = Command::new(BIN)
        .args([
            "admin",
            "-c",
            &file,
            "--no-open",
            "--print-url",
            "--port",
            "0",
        ])
        .current_dir(w.path())
        .env("BURROW_INSTANCE_DIR", w.file("inst-admin"))
        .output()
        .expect("burrow admin");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(
        err.contains(&format!(
            "visitor \"plain\" has no admin rights on {}",
            w.server_addr()
        )),
        "{err}"
    );
    assert!(err.contains("add \"admin: write\" (or \"admin: read\") to visitors.plain in server.yaml on the server"), "{err}");
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "no link is printed"
    );
}

async fn echo_server(port: u16) -> tokio::task::JoinHandle<()> {
    let l = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("echo port");
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    })
}

async fn round_trip(port: u16, bytes: usize) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let data = vec![7u8; bytes];
    let (mut r, mut w) = s.split();
    let send = async {
        w.write_all(&data).await?;
        w.shutdown().await
    };
    let mut got = Vec::new();
    let recv = r.read_to_end(&mut got);
    let (a, b) = tokio::join!(send, tokio::time::timeout(SECS, recv));
    a?;
    b.map_err(|_| std::io::Error::other("echo timed out"))??;
    Ok(())
}

fn visitor_live(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let echo = echo_server(c.world.ports["echo"]).await;
        open(c, "ops").await;
        c.ui.nav("/visitors/laptop").await;
        c.ui.wait_text("[data-testid=path-diagram]", "not seen since start", SECS)
            .await;
        c.ui.shot("visitor-not-seen-yet").await;
        let file = c.world.visitor_file("laptop");
        let (target, local) = (
            c.world.ports["echo"].to_string(),
            c.world.ports["forward"].to_string(),
        );
        c.world.start(
            "visit",
            &[
                "visit", "-c", &file, "--client", "home", "--target", &target, "--local", &local,
            ],
            &[],
        );
        let port = c.world.ports["forward"];
        let mut held = None;
        for _ in 0..50 {
            if let Ok(s) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                held = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let mut s = held.expect("the visit forward listens");
        s.write_all(b"ping").await.expect("send through the tunnel");
        let mut back = [0u8; 4];
        s.read_exact(&mut back)
            .await
            .expect("echo through the tunnel");
        c.ui.wait_text("[data-testid=path-diagram]", "in use now", SECS)
            .await;
        c.ui.wait_for(
            "[data-testid=entity-list] [data-row=laptop][data-group='In use now']",
            SECS,
        )
        .await;
        c.ui.shot("visitor-in-use-now").await;
        drop(s);
        c.ui.wait_text("[data-testid=path-diagram]", "no open connection", SECS)
            .await;
        c.ui.shot("visitor-last-used").await;
        c.ui.nav("/").await;
        c.ui.wait_text("[data-testid=kpi-visitors]", "2 used since start", SECS)
            .await;
        echo.abort();
        c.ui.assert_no_errors();
    })
}

fn admin_session(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        let port = world::free_port();
        let url = c.world.open_admin_on("ops", port);
        c.ui.goto(&url).await;
        c.ui.wait_text("[data-testid=conn-pill]", "Connected to", SECS)
            .await;
        c.ui.wait_gone("[data-testid=loading]", Duration::from_secs(20))
            .await;
        c.world.stop("admin-ops");
        c.ui.wait_text(
            "[data-testid=banner-reconnecting]",
            "burrow admin is not running",
            Duration::from_secs(20),
        )
        .await;
        c.ui.shot("admin-not-running").await;
        let log = c.world.file("admin-ops.log");
        let _ = std::fs::remove_file(c.world.file("url-ops"));
        let again = c.world.open_admin_on("ops", port);
        assert_eq!(
            again, url,
            "the next run for the same config reuses the session"
        );
        c.ui.wait_text(
            "[data-testid=conn-pill]",
            "Connected to",
            Duration::from_secs(20),
        )
        .await;
        c.ui.wait_gone("[data-testid=banner-reconnecting]", SECS)
            .await;
        world::wait_until("the reconnect note", Duration::from_secs(10), || {
            std::fs::read_to_string(&log)
                .is_ok_and(|t| t.contains("reconnected in your open browser tab"))
        });
        let printed = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            !printed.contains("#token="),
            "without --print-url the token is never printed: {printed}"
        );
        c.ui.shot("old-tab-reconnected").await;
        c.world.stop("admin-ops");
        c.ui.wait_text(
            "[data-testid=banner-reconnecting]",
            "burrow admin is not running",
            Duration::from_secs(20),
        )
        .await;
        tokio::time::sleep(Duration::from_secs(45)).await;
        let banner = c.ui.text("[data-testid=banner-reconnecting]").await;
        assert!(
            banner.contains("this tab reconnects") && !banner.contains("0 pending"),
            "45 s after burrow admin stopped the tab still retries every 2 s: {banner}"
        );
        c.ui.shot("admin-gone-for-a-while").await;
        let logged = c.ui.take_errors();
        assert!(
            !logged.iter().any(|l| l.contains("not sent")),
            "no console noise while disconnected: {logged:?}"
        );
        let _ = std::fs::remove_file(c.world.file("url-ops"));
        c.world.open_admin_on("ops", port);
        world::wait_until(
            "the reconnect note after a long outage",
            Duration::from_secs(10),
            || {
                std::fs::read_to_string(&log)
                    .is_ok_and(|t| t.contains("reconnected in your open browser tab"))
            },
        );
        let token = url.split("#token=").nth(1).unwrap().to_string();
        let foreign = url.replace(&token, &"0".repeat(64));
        c.ui.goto("about:blank").await;
        c.ui.goto(&foreign).await;
        c.ui.wait_text(
            "[data-testid=ended]",
            "This tab's admin session ended",
            SECS,
        )
        .await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(
            c.ui.count("[data-testid=banner-reconnecting]").await,
            0,
            "an ended session does not retry"
        );
        c.ui.shot("admin-session-ended").await;
        c.ui.take_errors();
    })
}

#[test]
fn an_old_tab_reconnects_to_the_next_burrow_admin_and_a_foreign_one_is_told_it_ended() {
    run("admin-session", Options::default(), false, admin_session);
}

fn old_server(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "ops").await;
        let ui = env!("CARGO_PKG_VERSION");
        let item = "[data-testid=attention] li[data-item=old-server]";
        let text = c.ui.wait_text(item, "v0.1.0", SECS).await;
        assert!(
            text.contains(&format!("older than this burrow admin (v{ui})"))
                && text.contains("sudo burrow update"),
            "{text}"
        );
        assert_eq!(
            c.ui.eval::<String>(&format!(
                "document.querySelector('{item}').getAttribute('data-tone')"
            ))
            .await,
            "warning"
        );
        let rows = c.ui.count("[data-testid=attention] li").await;
        let badge: String =
            c.ui.eval(
                "document.querySelector('[data-testid=attention] [data-testid=count]').textContent",
            )
            .await;
        assert_eq!(
            badge,
            rows.to_string(),
            "the older server counts in Needs attention"
        );
        assert_eq!(
            c.ui.count("[data-testid=old-server]").await,
            0,
            "no banner under the header"
        );
        c.ui.shot("older-server-attention").await;
        c.ui.nav("/about").await;
        c.ui.wait_text("[data-testid=about]", "older than this burrow admin", SECS)
            .await;
        c.ui.shot("older-server-about").await;
        c.ui.assert_no_errors();
    })
}

fn same_version_server(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.wait_for("[data-testid=attention] [data-testid=count]", SECS)
            .await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(
            c.ui.count("[data-testid=attention] li[data-item=old-server]")
                .await,
            0,
            "a server on this burrow admin's version needs no update"
        );
        c.ui.nav("/about").await;
        let about =
            c.ui.wait_text("[data-testid=about]", env!("CARGO_PKG_VERSION"), SECS)
                .await;
        assert!(!about.contains("older than this burrow admin"), "{about}");
        c.ui.assert_no_errors();
    })
}

#[test]
fn an_older_server_needs_attention_until_it_is_updated() {
    let opts = Options {
        server_env: vec![("BURROW_ADMIN_TEST_VERSION".into(), "0.1.0".into())],
        ..Options::default()
    };
    run("old-server", opts, true, old_server);
}

#[test]
fn a_server_on_the_same_version_needs_no_update() {
    run(
        "same-version",
        Options::default(),
        true,
        same_version_server,
    );
}

#[test]
fn a_visitor_shows_live_use_while_burrow_visit_carries_a_connection() {
    run("visitor-live", Options::default(), false, visitor_live);
}
