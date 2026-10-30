use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, open, run};

const ROW: &str = "[data-testid=filter-chips]";
const WIDTHS: [(i64, &str); 4] = [
    (360, "360"),
    (390, "390"),
    (768, "tablet"),
    (1440, "desktop"),
];

async fn one_line(c: &Ctx, what: &str, w: i64) {
    let tops: Vec<f64> = c
        .ui
        .eval(&format!("[...document.querySelectorAll('{ROW} [data-filter]')].filter(b => b.offsetParent).map(b => Math.round(b.getBoundingClientRect().top))"))
        .await;
    assert!(
        tops.len() > 1 && tops.iter().all(|t| *t == tops[0]),
        "{what} at {w}px: the chips share one line: {tops:?}"
    );
    let page_wide: bool =
        c.ui.eval("document.documentElement.scrollWidth > document.documentElement.clientWidth")
            .await;
    if page_wide {
        let wide: Vec<String> = c
            .ui
            .eval("[...document.querySelectorAll('body *')].filter(e => e.getBoundingClientRect().right > document.documentElement.clientWidth + 1).slice(0, 8).map(e => e.tagName + '.' + (e.getAttribute('class') || '').slice(0, 80) + ' ' + Math.round(e.getBoundingClientRect().right))")
            .await;
        panic!("{what} at {w}px: the page scrolls sideways: {wide:#?}");
    }
    let tall: f64 = c
        .ui
        .eval(&format!("Math.min(...[...document.querySelectorAll('{ROW} [data-filter]')].filter(b => b.offsetParent).map(b => b.getBoundingClientRect().height))"))
        .await;
    assert!(
        tall >= 24.0,
        "{what} at {w}px: chips stay 24 px tall (WCAG 2.5.8): {tall}"
    );
}

async fn last_chip_scrolls_into_view(c: &mut Ctx, what: &str, w: i64) {
    let (fits, fade): (bool, String) = c
        .ui
        .eval(&format!("(() => {{ const r = document.querySelector('{ROW}'); r.scrollLeft = 0; return [r.scrollWidth <= r.clientWidth + 1, r.dataset.fade]; }})()"))
        .await;
    if fits {
        return;
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let fade: String = if fade == "true" {
        fade
    } else {
        c.ui.eval(&format!("document.querySelector('{ROW}').dataset.fade"))
            .await
    };
    assert_eq!(fade, "true", "{what} at {w}px: the cut edge fades");
    let last: String =
        c.ui.eval(&format!(
            "[...document.querySelectorAll('{ROW} [data-filter]')].pop().dataset.filter"
        ))
        .await;
    c.ui.click(&format!("{ROW} [data-filter={last}]")).await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let inside: bool = c
        .ui
        .eval(&format!("(() => {{ const r = document.querySelector('{ROW}').getBoundingClientRect(); const b = document.querySelector('{ROW} [data-filter={last}]').getBoundingClientRect(); return b.left >= r.left - 1 && b.right <= r.right + 1; }})()"))
        .await;
    assert!(
        inside,
        "{what} at {w}px: the active chip {last} is scrolled into view"
    );
    c.ui.shot(&format!("{what}-chips-{w}-last")).await;
    c.ui.click(&format!("{ROW} [data-filter=all]")).await;
}

fn chips(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        for (what, path) in [
            ("services", "/services"),
            ("clients", "/clients"),
            ("visitors", "/visitors"),
            ("settings", "/settings"),
        ] {
            for (w, name) in WIDTHS {
                c.ui.viewport(w, 900).await;
                c.ui.nav(path).await;
                c.ui.wait_for(&format!("{ROW} [data-filter]"), SECS).await;
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                one_line(c, what, w).await;
                c.ui.shot(&format!("{what}-chips-{name}")).await;
                last_chip_scrolls_into_view(c, what, w).await;
            }
        }
        c.ui.viewport(360, 900).await;
        for path in ["/activity", "/settings", "/"] {
            c.ui.nav(path).await;
            c.ui.wait_for("[data-testid=nav-row] [aria-current=page]", SECS)
                .await;
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            let (one, seen): (bool, bool) = c
                .ui
                .eval("(() => { const r = document.querySelector('[data-testid=nav-row]'); const a = [...r.querySelectorAll('a')].map(x => Math.round(x.getBoundingClientRect().top)); const b = r.getBoundingClientRect(), on = r.querySelector('[aria-current=page]').getBoundingClientRect(); return [a.every(t => t === a[0]), on.left >= b.left - 1 && on.right <= b.right + 1]; })()")
                .await;
            assert!(one, "the top navigation is one line at 360 px on {path}");
            assert!(seen, "the current section is in view at 360 px on {path}");
            c.ui.shot(&format!("nav-360{}", path.replace('/', "-")))
                .await;
        }
        c.ui.viewport(1440, 900).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn filter_chips_stay_on_one_line_and_scroll_sideways() {
    let opts = Options {
        extra_top: "web:\n  https: 127.0.0.1:HTTPS_PORT\n".into(),
        extra_services:
            "  dns:\n    client: home\n    protocol: udp\n    target: 53\n    visitors: [laptop]\n"
                .into(),
        ..Options::default()
    };
    run("filter-chips", opts, false, chips);
}
