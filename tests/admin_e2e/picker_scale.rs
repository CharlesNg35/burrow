use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, edit_mode, open, run};

const FIELD: &str = "#login-service";
const LIST: &str = "#login-service-list";
const DIALOG: &str = "[data-testid=login-dialog]";
const TRAFFIC: &str = "#traffic-service";
const MANY: usize = 5000;
const ALL: usize = MANY + 2;
const CLIENTS: usize = 500;

fn opts() -> Options {
    Options {
        extra_top: "web:\n  http: 127.0.0.1:HTTP_PORT\n".into(),
        extra_services: (1..=MANY)
            .map(|i| {
                format!(
                    "  app{i:04}:\n    client: box{:03}\n    target: {}\n    visitors: [laptop]\n",
                    i % CLIENTS + 1,
                    10000 + i
                )
            })
            .collect(),
        extra_clients: (1..=CLIENTS)
            .map(|i| format!("  box{i:03}:\n    secret: {}\n", super::world::secret()))
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
    assert_eq!(before.lit, "app0001", "{step}: the first service is lit");
    for _ in 0..30 {
        c.ui.press(FIELD, "ArrowDown").await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let now = scroll(c).await;
    assert_eq!(
        now.lit, "app0031",
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
    assert_eq!(back.lit, "app0001", "{step}: arrows go back up");
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
    type_in(c, FIELD, "app421").await;
    c.ui.wait_true(
        &format!("document.querySelectorAll('{LIST} [role=option]').length === 10 && !document.querySelector('{LIST} [data-more]')"),
        "typing narrows the list to app4210 to app4219",
        SECS,
    )
    .await;
    c.ui.press(FIELD, "ArrowDown").await;
    c.ui.press(FIELD, "ArrowDown").await;
    c.ui.press(FIELD, "Enter").await;
    c.ui.wait_gone(LIST, SECS).await;
    c.ui.wait_for(&format!("{DIALOG} [data-chip=app4212]"), SECS)
        .await;
}

fn shown(total: usize) -> String {
    let more = if total > 100 {
        format!(
            "m && m.textContent === '{} more, type to narrow'",
            total - 100
        )
    } else {
        "!m".to_string()
    };
    format!(
        "(() => {{ const m = document.querySelector('{LIST} [data-more]'); return document.querySelectorAll('{LIST} [role=option]').length === {} && {more}; }})()",
        total.min(100)
    )
}

async fn timed(c: &Ctx, act: &str, done: &str) -> f64 {
    let ms: f64 = c
        .ui
        .eval(&format!(
            "(async () => {{ const t0 = performance.now(); {act}; for (let i = 0; i < 20000 && !({done}); i++) await new Promise(r => setTimeout(r, 0)); if (!({done})) return -1; return performance.now() - t0; }})()"
        ))
        .await;
    assert!(ms >= 0.0, "timed out waiting for {done}");
    ms
}

fn key(k: &str) -> String {
    format!(
        "document.querySelector('{FIELD}').dispatchEvent(new KeyboardEvent('keydown', {{ key: '{k}', bubbles: true, cancelable: true }}))"
    )
}

fn typed(text: &str) -> String {
    format!(
        "(() => {{ const e = document.querySelector('{FIELD}'); e.value = '{text}'; e.dispatchEvent(new Event('input', {{ bubbles: true }})); }})()"
    )
}

#[derive(Debug, Default, serde::Serialize)]
struct Timings {
    open_ms: f64,
    keystroke_ms: Vec<(String, f64)>,
    arrow_ms: Vec<f64>,
}

async fn measure_keys(c: &Ctx, t: &mut Timings) {
    for (text, total) in [
        ("a", MANY),
        ("app4", 1000),
        ("app42", 100),
        ("app421", 10),
        ("", ALL),
    ] {
        let ms = timed(c, &typed(text), &shown(total)).await;
        t.keystroke_ms.push((text.to_string(), ms));
    }
    for i in 1..=10 {
        let ms = timed(
            c,
            &key("ArrowDown"),
            &format!("document.getElementById('login-service-opt-{i}').classList.contains('bg-surface-3')"),
        )
        .await;
        t.arrow_ms.push(ms);
    }
    for _ in 0..10 {
        c.ui.press(FIELD, "ArrowUp").await;
    }
}

fn save_timings(c: &Ctx, t: &Timings) {
    let text = serde_json::to_string_pretty(t).expect("timings as JSON");
    std::fs::write(c.ui.flow.dir.join("timings.json"), &text).expect("write the timings");
    println!("picker timings with {ALL} services: {text}");
}

async fn login_picker(c: &mut Ctx) {
    c.ui.nav("/settings?tab=proxy").await;
    edit_mode(c).await;
    c.ui.click("[data-testid=login-add]").await;
    c.ui.wait_for(&format!("{DIALOG} {FIELD}"), SECS).await;
    let mut t = Timings {
        open_ms: timed(
            c,
            &format!("document.querySelector('{FIELD}').focus()"),
            &shown(ALL),
        )
        .await,
        ..Timings::default()
    };
    long_list_is_capped(c).await;
    measure_keys(c, &mut t).await;
    save_timings(c, &t);
    arrows_stay_put(c, "desktop").await;
    c.ui.view_shot("picker-scale-login").await;
    type_and_pick(c).await;
    c.ui.viewport(390, 844).await;
    type_in(c, FIELD, "").await;
    open_picker(c).await;
    c.ui.wait_true(
        &format!("!!document.querySelector('{LIST} [data-option=app4212][aria-selected=true]')"),
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
    c.ui.wait_for("#traffic-service-list [data-option=app0001]", SECS)
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert_eq!(
        c.ui.count("#traffic-service-list").await,
        1,
        "the open list survives the live status ticks"
    );
    type_in(c, TRAFFIC, "app0007").await;
    c.ui.wait_true(
        "document.querySelectorAll('#traffic-service-list [role=option]').length === 1",
        "typing narrows the services",
        SECS,
    )
    .await;
    c.ui.press(TRAFFIC, "Enter").await;
    c.ui.wait_for("[data-testid=traffic] [data-chip=app0007]", SECS)
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
fn a_picker_over_5000_services_filters_scrolls_only_its_list_and_picks_by_keyboard() {
    run("picker-scale", opts(), false, picker_scale);
}
