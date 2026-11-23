use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, edit_mode, open, run};

const FIELD: &str = "#login-service";
const LIST: &str = "#login-service-list";
const DIALOG: &str = "[data-testid=login-dialog]";
const TRAFFIC: &str = "#traffic-service";

fn opts() -> Options {
    Options {
        extra_top: "web:\n  http: 127.0.0.1:HTTP_PORT\n".into(),
        extra_services: (1..=500)
            .map(|i| format!("  app{i:03}:\n    client: home\n    target: {}\n", 9000 + i))
            .collect(),
        ..Options::default()
    }
}

#[derive(Debug, serde::Deserialize, PartialEq)]
struct Scroll {
    page: f64,
    dialog: f64,
    list: f64,
    lit: String,
    in_view: bool,
}

async fn scroll(c: &Ctx) -> Scroll {
    c.ui.eval(&format!(
        "(() => {{ const l = document.querySelector('{LIST}'); const d = document.querySelector('{DIALOG}'); const lit = l.querySelector('[role=option].bg-surface-3'); const lr = l.getBoundingClientRect(); const r = lit ? lit.getBoundingClientRect() : lr; return {{ page: scrollY, dialog: d.scrollTop, list: l.scrollTop, lit: lit ? lit.dataset.option : '', in_view: r.top >= lr.top - 0.5 && r.bottom <= lr.bottom + 0.5 }}; }})()"
    ))
    .await
}

async fn type_in(c: &Ctx, css: &str, text: &str) {
    let _: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const e = document.querySelector('{css}'); e.value = '{text}'; e.dispatchEvent(new Event('input', {{ bubbles: true }})); return true; }})()"
        ))
        .await;
}

async fn open_picker(c: &Ctx) {
    c.ui.click(FIELD).await;
    c.ui.wait_for(&format!("{LIST} [data-option]"), SECS).await;
}

async fn arrows_stay_put(c: &Ctx, step: &str) {
    let before = scroll(c).await;
    assert_eq!(before.lit, "app001", "{step}: the first service is lit");
    for _ in 0..30 {
        c.ui.press(FIELD, "ArrowDown").await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let now = scroll(c).await;
    assert_eq!(
        now.lit, "app031",
        "{step}: 30 arrows light the 31st service"
    );
    assert!(
        now.in_view,
        "{step}: the lit row is in the list's view: {now:?}"
    );
    assert!(
        now.list > before.list,
        "{step}: the list scrolls: {before:?} -> {now:?}"
    );
    assert!(
        (now.page - before.page).abs() < 0.5 && (now.dialog - before.dialog).abs() < 0.5,
        "{step}: the page and the dialog stay put: {before:?} -> {now:?}"
    );
    for _ in 0..30 {
        c.ui.press(FIELD, "ArrowUp").await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let back = scroll(c).await;
    assert_eq!(back.lit, "app001", "{step}: arrows go back up");
    assert!(
        back.in_view && back.list < 0.5,
        "{step}: the list scrolls back: {back:?}"
    );
    assert!(
        (back.page - before.page).abs() < 0.5 && (back.dialog - before.dialog).abs() < 0.5,
        "{step}: the page and the dialog stay put going up: {before:?} -> {back:?}"
    );
}

async fn long_list_is_capped(c: &Ctx) {
    assert_eq!(
        c.ui.count(&format!("{LIST} [role=option]")).await,
        100,
        "at most 100 rows are drawn"
    );
    let more = c.ui.text(&format!("{LIST} [data-more]")).await;
    assert!(more.contains("more, type to narrow"), "{more}");
}

async fn type_and_pick(c: &Ctx) {
    type_in(c, FIELD, "app42").await;
    c.ui.wait_true(
        &format!("document.querySelectorAll('{LIST} [role=option]').length === 10 && !document.querySelector('{LIST} [data-more]')"),
        "typing narrows the list to app420 to app429",
        SECS,
    )
    .await;
    c.ui.press(FIELD, "ArrowDown").await;
    c.ui.press(FIELD, "ArrowDown").await;
    c.ui.press(FIELD, "Enter").await;
    c.ui.wait_gone(LIST, SECS).await;
    c.ui.wait_for(&format!("{DIALOG} [data-chip=app422]"), SECS)
        .await;
}

async fn login_picker(c: &mut Ctx) {
    c.ui.nav("/settings?tab=proxy").await;
    edit_mode(c).await;
    c.ui.click("[data-testid=login-add]").await;
    c.ui.wait_for(&format!("{DIALOG} {FIELD}"), SECS).await;
    open_picker(c).await;
    long_list_is_capped(c).await;
    arrows_stay_put(c, "desktop").await;
    c.ui.view_shot("picker-scale-login").await;
    type_and_pick(c).await;
    c.ui.viewport(390, 844).await;
    type_in(c, FIELD, "").await;
    open_picker(c).await;
    c.ui.wait_true(
        &format!("!!document.querySelector('{LIST} [data-option=app422][aria-selected=true]')"),
        "the pick is marked in the list",
        SECS,
    )
    .await;
    c.ui.view_shot("picker-scale-login-phone").await;
    c.ui.press(FIELD, "Escape").await;
    c.ui.wait_gone(LIST, SECS).await;
    c.ui.viewport(1440, 900).await;
    c.ui.click(&format!("{DIALOG} button[aria-label=Close]"))
        .await;
    c.ui.wait_gone(DIALOG, SECS).await;
    c.ui.click("[data-testid=discard-edit]").await;
}

async fn traffic_picker(c: &mut Ctx) {
    c.ui.nav("/").await;
    c.ui.click(TRAFFIC).await;
    c.ui.wait_for("#traffic-service-list [data-option=app001]", SECS)
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert_eq!(
        c.ui.count("#traffic-service-list").await,
        1,
        "the open list survives the live status ticks"
    );
    type_in(c, TRAFFIC, "app007").await;
    c.ui.wait_true(
        "document.querySelectorAll('#traffic-service-list [role=option]').length === 1",
        "typing narrows the services",
        SECS,
    )
    .await;
    c.ui.press(TRAFFIC, "Enter").await;
    c.ui.wait_for("[data-testid=traffic] [data-chip=app007]", SECS)
        .await;
}

fn picker_scale(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.viewport(1440, 900).await;
        login_picker(c).await;
        traffic_picker(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_picker_over_500_services_filters_scrolls_only_its_list_and_picks_by_keyboard() {
    run("picker-scale", opts(), false, picker_scale);
}
