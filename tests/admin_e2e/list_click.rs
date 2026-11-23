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
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    c.ui.eval(&format!(
        "(() => {{ const l = document.querySelector('{LIST}'); const b = document.querySelector('[data-row=\"{key}\"]').getBoundingClientRect(); return {{ key: '{key}', x: b.left + 40, y: b.top + 20, list: l.scrollTop, page: scrollY }}; }})()"
    ))
    .await
}

async fn click_stays(c: &mut Ctx, nth: usize, step: &str) {
    let before = spot(c, nth).await;
    let _: bool = c.ui.eval(&format!("(() => {{ const l = document.querySelector('{LIST}'); l.__mark = 1; window.__log = []; const t0 = performance.now(); const tick = () => { const n = document.querySelector('{LIST}'); window.__log.push(Math.round(performance.now()-t0) + ':' + (n === l ? 'old' : 'new') + ':' + n.scrollTop + ':' + n.scrollHeight + ':' + n.querySelectorAll('[data-row]').length); if (performance.now() - t0 < 500) requestAnimationFrame(tick); }; addEventListener('pointerdown', () => requestAnimationFrame(tick), {{ once: true }}); return true; }})()")).await;
    c.ui.click_at(before.x, before.y).await;
    let now = after(c, &before.key).await;
    let log: String = c.ui.eval(&format!("(() => {{ const l = document.querySelector('{LIST}'); return 'same=' + !!l.__mark + ' lists=' + document.querySelectorAll('{LIST}').length + ' log=' + window.__log.join('|'); }})()")).await;
    println!("{step}: {log}");
    println!("{step}: before {before:?} after {now:?}");
    assert!(
        (now.y - before.y).abs() < 1.0 && (now.list - before.list).abs() < 1.0,
        "{step}: a row in view stays under the pointer when clicked: {before:?} -> {now:?}"
    );
}

fn list_click(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.viewport(1440, 900).await;
        c.ui.nav("/services").await;
        c.ui.wait_for("[data-row=app40]", SECS).await;
        click_stays(c, 2, "top of the list").await;
        let _: bool = c
            .ui
            .eval(&format!("(() => {{ document.querySelector('{LIST}').scrollTop = 900; return true; }})()"))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        click_stays(c, 1, "list scrolled").await;
        click_stays(c, 99, "last row in view").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_clicked_row_stays_where_it_was() {
    run("list-click", opts(), false, list_click);
}
