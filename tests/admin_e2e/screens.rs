use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use super::checks::ax_names;
use super::world;
use super::world::Options;
use super::{Ctx, SECS, ax_clean, baseline, edit_mode, open, run};

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
        for (path, name) in MAIN_SCREENS {
            c.ui.nav(path).await;
            c.ui.wait_gone("[aria-busy=true]", SECS).await;
            c.ui.shot(&format!("{name}-phone")).await;
            baseline(c, &format!("{name}-phone")).await;
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
            "../../crates/burrow-admin-proto/tests/fixtures/v0.1/schema.json"
        ))
        .expect("the frozen schema");
        let mut wanted = Vec::new();
        leaves(&schema["root"], Vec::new(), &mut wanted);
        open(c, "ops").await;
        let mut seen: Vec<String> = Vec::new();
        let mut unset: Vec<String> = Vec::new();
        for (path, tabs) in [
            (
                "/settings",
                &["server", "connections", "proxy", "monitoring"][..],
            ),
            ("/clients/office", &["settings", "secrets"][..]),
            ("/visitors/ops", &["access", "secrets"][..]),
            ("/visitors/laptop", &["access"][..]),
            ("/services/web", &["settings"][..]),
            ("/services/game", &["settings"][..]),
            ("/services/dns", &["settings"][..]),
            ("/services/nas", &["settings"][..]),
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
            "  game:\n    clients: [home, office]\n    mode: balance\n    health_check: true\n    bind: 127.0.0.1:{p}\n    target: 7000\n    max_connections: 9\n    allow_from: [127.0.0.1/32]\n    deny_from: [10.0.0.0/8]\n    proxy_protocol: v1\n    bandwidth: 5MB\n  dns:\n    client: office\n    protocol: udp\n    bind: 127.0.0.1:{q}\n    target: 53\n  nas:\n    client: home\n    target: 8443\n    web: {{ hosts: [nas.example.com], tls: passthrough }}\n",
            p = world::free_port(),
            q = world::free_port()
        ),
        ..Options::default()
    };
    let opts = Options {
        extra_top: "web:\n  https: 127.0.0.1:HTTPS_PORT\n".into(),
        ..opts
    };
    run("schema-walk", opts, false, schema_walk);
}
