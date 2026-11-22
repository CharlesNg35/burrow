use super::edits::Flow;
use super::text_edit::{EDITOR, start};
use super::world::Options;
use super::{Ctx, SECS, open, run};

const FULL: &str = "[data-testid=file-full]";
const HEAD: &str = "[data-testid=settings-head]";
const VIEW: &str = "[data-testid=file-text]";

fn opts() -> Options {
    Options {
        extra_services: (1..=60)
            .map(|i| {
                format!(
                    "  app{i:02}:\n    client: home\n    target: {}\n    visitors: [laptop]\n",
                    9000 + i
                )
            })
            .collect(),
        ..Options::default()
    }
}

#[derive(Debug, serde::Deserialize)]
struct Fit {
    bottom: f64,
    page: f64,
    window: f64,
    head: bool,
    /// From the box up to the page: each element's test id or tag, height and display, for a failure.
    chain: Vec<String>,
}

async fn fit(c: &Ctx, sel: &str) -> Fit {
    c.ui.eval(&format!(
        "(() => {{ const b = document.querySelector('{sel}').getBoundingClientRect(); const h = document.querySelector('{HEAD}'); return {{ bottom: b.bottom + scrollY, page: document.documentElement.scrollHeight, window: innerHeight, head: !!(h && h.offsetParent), chain: (() => {{ const out = []; for (let e = document.querySelector('{sel}'); e && e !== document.body; e = e.parentElement) {{ const s = getComputedStyle(e); [...e.children].forEach(k => {{ const r = k.getBoundingClientRect(); if (r.height > 40 && !k.contains(document.querySelector('{sel}'))) out.push('  sibling ' + (k.dataset.testid || k.tagName) + ' ' + Math.round(r.height)); }}); out.push((e.dataset.testid || e.tagName + '.' + e.className.toString().slice(0, 40)) + ' h=' + Math.round(e.getBoundingClientRect().height) + ' ' + s.display + ' flex=' + s.flex); }} return out; }})() }}; }})()"
    ))
    .await
}

/// The box ends a little above the window's bottom (the page's padding), and the page does not scroll.
async fn fills(c: &Ctx, sel: &str, step: &str) -> Fit {
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let f = fit(c, sel).await;
    assert!(
        f.page <= f.window + 1.0,
        "{step}: the page fits the window, only the text scrolls: {f:?}"
    );
    assert!(
        f.bottom >= f.window - 48.0 && f.bottom <= f.window,
        "{step}: the text's box takes the height left, down to the bottom: {f:?}"
    );
    f
}

async fn full(c: &Ctx, on: bool) {
    c.ui.click(FULL).await;
    c.ui.wait_true(
        &format!("document.querySelector('{FULL}').getAttribute('aria-pressed') === '{on}'"),
        "the button says whether full screen is on",
        SECS,
    )
    .await;
    let label = c.ui.attr_of(FULL, "aria-label").await;
    let want = if on {
        "Leave full screen"
    } else {
        "Full screen"
    };
    assert_eq!(label, want, "the button names what it does");
}

async fn read_view(c: &mut Ctx) {
    c.ui.viewport(1440, 900).await;
    c.ui.nav("/settings?tab=file").await;
    c.ui.wait_text(VIEW, "app60", SECS).await;
    let normal = fills(c, VIEW, "read at 1440").await;
    assert!(
        normal.head,
        "the page header and the Settings tabs are shown"
    );
    full(c, true).await;
    let f = fills(c, VIEW, "read in full screen").await;
    assert!(
        !f.head,
        "full screen hides the page header and the Settings tabs: {f:?}"
    );
    assert_eq!(
        c.ui.count("[data-testid=header]").await,
        1,
        "the app's own header stays"
    );
    c.ui.view_shot("text-fill-read-full").await;
    c.ui.viewport(390, 844).await;
    fills(c, VIEW, "read in full screen on a phone").await;
    c.ui.view_shot("text-fill-read-full-phone").await;
    c.ui.viewport(1440, 900).await;
    full(c, false).await;
    assert!(
        fit(c, VIEW).await.head,
        "leaving full screen shows them again"
    );
}

async fn editor(c: &mut Ctx) {
    start(c).await;
    fills(c, EDITOR, "edit at 1440").await;
    c.ui.view_shot("text-fill-edit").await;
    full(c, true).await;
    fills(c, EDITOR, "edit in full screen").await;
    c.ui.view_shot("text-fill-edit-full").await;
    c.ui.viewport(390, 844).await;
    fills(c, EDITOR, "edit in full screen on a phone").await;
    c.ui.view_shot("text-fill-edit-full-phone").await;
    c.ui.viewport(1440, 900).await;
}

async fn leaving_ends_full_screen(c: &mut Ctx) {
    c.ui.click("[data-testid=file-cancel]").await;
    c.ui.nav("/").await;
    c.ui.nav("/settings?tab=file").await;
    c.ui.wait_text(VIEW, "app60", SECS).await;
    assert_eq!(
        c.ui.attr_of(FULL, "aria-pressed").await,
        "false",
        "coming back to Settings shows the whole page"
    );
    assert!(fit(c, VIEW).await.head);
}

fn text_fill(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        read_view(c).await;
        editor(c).await;
        leaving_ends_full_screen(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn raw_config_fills_the_window_and_has_a_full_screen() {
    run("text-fill", opts(), false, text_fill);
}
