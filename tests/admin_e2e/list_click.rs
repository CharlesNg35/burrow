use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, open, run};

const LIST: &str = "[data-testid=list]";

fn opts() -> Options {
    Options {
        extra_services: (1..=40)
            .map(|i| {
                format!(
                    "  app{i:02}:\n    client: home\n    target: {}\n    visitors: [laptop]\n",
                    9000 + i
                )
            })
            .collect(),
        extra_clients: (1..=40)
            .map(|i| format!("  box{i:02}:\n    secret: {}\n", super::world::secret()))
            .collect(),
        ..Options::default()
    }
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
        "(() => {{ const l = document.querySelector('{LIST}'); const lr = l.getBoundingClientRect(); const rows = [...l.querySelectorAll('[data-row]')].filter(r => {{ const b = r.getBoundingClientRect(); return b.height > 0 && b.top >= lr.top && b.bottom <= lr.bottom; }}); const r = rows[Math.min({nth}, rows.length - 1)]; const b = r.getBoundingClientRect(); return {{ key: r.dataset.row, x: b.left + 40, y: b.top + 20, list: l.scrollTop, page: scrollY }}; }})()"
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
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
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
        (now.y - before.y).abs() < 1.0 && (now.list - before.list).abs() < 1.0,
        "{step}: a row in view stays under the pointer when clicked: {before:?} -> {now:?}"
    );
}

async fn scroll_list(c: &Ctx, top: u32) {
    let _: bool =
        c.ui.eval(&format!(
            "(() => {{ document.querySelector('{LIST}').scrollTop = {top}; return true; }})()"
        ))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

async fn page(c: &mut Ctx, path: &str, last: &str, what: &str) {
    c.ui.nav(path).await;
    c.ui.wait_for(&format!("[data-row={last}]"), SECS).await;
    click_stays(c, 2, &format!("{what}: top of the list")).await;
    scroll_list(c, 900).await;
    click_stays(c, 1, &format!("{what}: list scrolled")).await;
    click_stays(c, 99, &format!("{what}: last row in view")).await;
}

fn list_click(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.viewport(1440, 900).await;
        page(c, "/services", "app40", "services").await;
        page(c, "/clients", "box40", "clients").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_clicked_row_stays_where_it_was() {
    run("list-click", opts(), false, list_click);
}
