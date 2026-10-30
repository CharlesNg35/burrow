use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use super::world;
use super::world::Options;
use super::{Ctx, SECS, echo_server, open, round_trip, run};

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
        let tip = "[data-testid=flow] [data-testid=tooltip]";
        c.ui.wait_for(tip, SECS).await;
        let left: String =
            c.ui.eval(&format!("document.querySelector('{tip}').style.left"))
                .await;
        assert!(
            left.ends_with("px"),
            "the flow tooltip is placed without an inline style attribute: {left:?}"
        );
        c.ui.shot("flow-chart").await;
        c.ui.dark(true).await;
        let ratio: f64 = c
            .ui
            .eval(&format!("(() => {{ const s = getComputedStyle(document.querySelector('{tip}')); const l = c => {{ const v = c.match(/[\\d.]+/g).slice(0, 3).map(x => {{ const k = x / 255; return k <= 0.03928 ? k / 12.92 : ((k + 0.055) / 1.055) ** 2.4; }}); return 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]; }}; const a = l(s.color), b = l(s.backgroundColor); return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05); }})()"))
            .await;
        assert!(ratio >= 4.5, "the tooltip reads in dark: {ratio:.2}:1");
        c.ui.shot("flow-chart-dark").await;
        c.ui.dark(false).await;
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
            let sideways: Vec<String> = c
                .ui
                .eval("[...document.querySelectorAll('[data-testid=served] *')].filter(e => e.offsetParent !== null && ['auto', 'scroll'].includes(getComputedStyle(e).overflowX) && e.scrollWidth > e.clientWidth + 1).map(e => e.className)")
                .await;
            assert!(
                sideways.is_empty(),
                "at {w} px the services home serves scroll sideways: {sideways:?}"
            );
            let targets: Vec<bool> = c
                .ui
                .eval("[...document.querySelectorAll('[data-testid=served] [data-target]')].filter(e => e.offsetParent !== null).map(e => { const r = e.getBoundingClientRect(); return r.width > 0 && r.right <= document.documentElement.clientWidth; })")
                .await;
            assert!(
                !targets.is_empty() && targets.iter().all(|t| *t),
                "at {w} px every served target shows on screen: {targets:?}"
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

fn old_server(c: &mut Ctx) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    Box::pin(async move {
        open(c, "ops").await;
        let ui = env!("CARGO_PKG_VERSION");
        let item = "[data-testid=attention] li[data-item=old-server]";
        let text = c.ui.wait_text(item, "v0.1.0", SECS).await;
        assert!(
            text.contains(&format!("older than this burrow admin (v{ui})"))
                && text.contains("burrow update on the server as root or administrator"),
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
