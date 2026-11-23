use std::time::Duration;

use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, edit_mode, open, open_more, run, wait_checked};

const LIST: &str = "[data-testid=list]";
const ROWS: usize = 40;
const KEPT: usize = 20;

fn opts() -> Options {
    Options {
        extra_services: (1..=ROWS)
            .map(|i| {
                format!(
                    "  app{i:02}:\n    client: home\n    target: {}\n    visitors: [laptop]\n",
                    9000 + i
                )
            })
            .collect(),
        extra_clients: (1..=ROWS)
            .map(|i| format!("  box{i:02}:\n    secret: {}\n", super::world::secret()))
            .collect(),
        drop_ins: vec![(
            "people.yaml",
            format!(
                "visitors:\n{}",
                (1..=ROWS)
                    .map(|i| format!("  v{i:02}:\n    secret: {}\n", super::world::secret()))
                    .collect::<String>()
            ),
            false,
        )],
        files: kept_versions(),
        ..Options::default()
    }
}

fn kept_versions() -> Vec<(String, String)> {
    let start: u128 = 1_700_000_000_000_000_000;
    (0..ROWS as u128)
        .map(|i| {
            (
                format!(
                    ".server.yaml.history/{}-{:016x}.yaml",
                    start + i * 60_000_000_000,
                    0xabc0_0000 + i
                ),
                "CONFIG".to_string(),
            )
        })
        .collect()
}

#[derive(Debug, serde::Deserialize)]
struct Spot {
    key: String,
    x: f64,
    y: f64,
    list: f64,
    page: f64,
}

async fn spot(c: &Ctx, nth: usize) -> Spot {
    c.ui.eval(&format!(
        "(() => {{ const l = document.querySelector('{LIST}'); const lr = l.getBoundingClientRect(); const rows = [...l.querySelectorAll('[data-row]')].filter(r => {{ const b = r.getBoundingClientRect(); return b.height > 0 && b.top >= Math.max(lr.top, 0) && b.bottom <= Math.min(lr.bottom, innerHeight); }}); const r = rows[Math.min({nth}, rows.length - 1)]; const b = r.getBoundingClientRect(); return {{ key: r.dataset.row, x: b.left + 40, y: b.top + 20, list: l.scrollTop, page: scrollY }}; }})()"
    ))
    .await
}

async fn after(c: &Ctx, key: &str) -> Spot {
    c.ui.wait_true(
        &format!("!!document.querySelector('[data-row=\"{key}\"][data-selected=true]')"),
        "the clicked row is selected",
        SECS,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    c.ui.eval(&format!(
        "(() => {{ const l = document.querySelector('{LIST}'); const b = document.querySelector('[data-row=\"{key}\"]').getBoundingClientRect(); return {{ key: '{key}', x: b.left + 40, y: b.top + 20, list: l.scrollTop, page: scrollY }}; }})()"
    ))
    .await
}

async fn click_stays(c: &mut Ctx, nth: usize, step: &str) {
    let before = spot(c, nth).await;
    c.ui.click_at(before.x, before.y).await;
    let now = after(c, &before.key).await;
    assert!(
        (now.y - before.y).abs() < 1.0
            && (now.list - before.list).abs() < 1.0
            && (now.page - before.page).abs() < 1.0,
        "{step}: a row in view stays under the pointer when clicked: {before:?} -> {now:?}"
    );
}

async fn scroll_list(c: &Ctx, top: u32) {
    let _: bool =
        c.ui.eval(&format!(
            "(() => {{ document.querySelector('{LIST}').scrollTop = {top}; return true; }})()"
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
}

async fn rows_shown(c: &Ctx, n: usize) {
    c.ui.wait_true(
        &format!("(() => {{ const w = document.querySelector('{LIST} [data-testid=list-window]'); return (w ? Number(w.dataset.count) : document.querySelectorAll('{LIST} [data-row]').length) >= {n}; }})()"),
        "the list shows every row",
        SECS,
    )
    .await;
}

async fn page(c: &mut Ctx, path: &str, rows: usize, what: &str) {
    c.ui.nav(path).await;
    rows_shown(c, rows).await;
    click_stays(c, 2, &format!("{what}: top of the list")).await;
    scroll_list(c, 900).await;
    click_stays(c, 1, &format!("{what}: list scrolled")).await;
    click_stays(c, 99, &format!("{what}: last row in view")).await;
}

async fn with_changes_pending(c: &mut Ctx) {
    c.ui.nav("/clients/home?tab=settings").await;
    edit_mode(c).await;
    open_more(c, "clients/*/max_connections").await;
    c.ui.fill("#f-clients-home-max-connections", "7").await;
    wait_checked(c).await;
    c.ui.click("[data-testid=save]").await;
    c.ui.wait_for("[data-testid=toast]", SECS).await;
    for width in [1440, 1100] {
        c.ui.viewport(width, 900).await;
        c.ui.nav("/services").await;
        rows_shown(c, ROWS).await;
        c.ui.wait_for("[data-testid=review-open]", SECS).await;
        scroll_list(c, 900).await;
        click_stays(c, 99, &format!("pending changes at {width}: lowest row")).await;
        click_stays(c, 98, &format!("pending changes at {width}: low row")).await;
    }
    c.ui.viewport(1440, 900).await;
}

async fn phone_back(c: &mut Ctx) {
    c.ui.viewport(390, 800).await;
    c.ui.nav("/services").await;
    rows_shown(c, ROWS).await;
    let _: bool =
        c.ui.eval("(() => { scrollTo(0, 1200); return true; })()")
            .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let before = spot(c, 2).await;
    assert!(
        before.page > 1000.0,
        "the phone list scrolls the page: {before:?}"
    );
    c.ui.click_at(before.x, before.y).await;
    c.ui.wait_for(
        &format!("[data-testid=detail][data-name={}]", before.key),
        SECS,
    )
    .await;
    let top: f64 = c.ui.eval("scrollY").await;
    assert!(top < 1.0, "a phone detail opens at its top: {top}");
    let _: bool =
        c.ui.eval("(() => { history.back(); return true; })()")
            .await;
    c.ui.wait_true(
        &format!(
            "(() => {{ const r = document.querySelector('[data-row=\"{}\"]'); return !!r && r.getBoundingClientRect().height > 0 && Math.abs(scrollY - {}) < 2; }})()",
            before.key, before.page
        ),
        "Back on a phone returns to the list where it was",
        SECS,
    )
    .await;
    let now = spot(c, 2).await;
    assert!(
        now.key == before.key && (now.y - before.y).abs() < 2.0,
        "Back puts the opened row where it was: {before:?} -> {now:?}"
    );
    c.ui.viewport(1440, 900).await;
}

async fn dialog_close_stays(c: &mut Ctx) {
    c.ui.viewport(1440, 520).await;
    c.ui.nav("/services/web").await;
    c.ui.wait_for("[data-testid=detail][data-name=web]", SECS)
        .await;
    c.ui.click("[data-testid=more]").await;
    c.ui.click("[data-testid=rename-open]").await;
    c.ui.wait_for("[data-testid=rename-entity] #rename-to", SECS)
        .await;
    let y: f64 = c
        .ui
        .eval("(() => { scrollTo(0, document.documentElement.scrollHeight); return scrollY; })()")
        .await;
    assert!(y > 100.0, "the detail scrolls under the dialog: {y}");
    c.ui.press("#rename-to", "Escape").await;
    c.ui.wait_gone("[data-testid=rename-entity]", SECS).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let now: f64 = c.ui.eval("scrollY").await;
    assert!(
        (now - y).abs() < 1.0,
        "closing a dialog leaves the page where it was: {y} -> {now}"
    );
    c.ui.viewport(1440, 900).await;
}

fn list_click(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.viewport(1440, 900).await;
        page(c, "/services", ROWS, "services").await;
        page(c, "/clients", ROWS, "clients").await;
        page(c, "/visitors", ROWS, "visitors").await;
        page(c, "/activity", KEPT, "activity").await;
        phone_back(c).await;
        dialog_close_stays(c).await;
        with_changes_pending(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_clicked_row_stays_where_it_was() {
    run("list-click", opts(), false, list_click);
}
