use std::time::Instant;

use super::edits::Flow;
use super::world::{self, Options};
use super::{Ctx, SECS, edit_mode, open, run};

const SERVICES: usize = 300;
const SITES: usize = 12;
const WIDE: usize = 12;
const VISITORS: usize = 200;
const SERVED: usize = SERVICES + SITES + 3;

fn opts() -> Options {
    let mut services = String::new();
    for i in 1..=SERVICES {
        services.push_str(&format!(
            "  s{i:03}:\n    client: home\n    target: {}\n    visitors: [laptop]\n",
            20000 + i
        ));
    }
    for i in 1..=SITES {
        services.push_str(&format!(
            "  site{i:02}:\n    client: home\n    target: {}\n    web: {{ hosts: [site{i:02}.localhost], tls: internal }}\n",
            21000 + i
        ));
    }
    let wide: Vec<String> = (1..=WIDE).map(|i| format!("c{i:02}")).collect();
    services.push_str(&format!(
        "  wide:\n    clients: [{}]\n    target: 22\n    visitors: [laptop]\n",
        wide.join(", ")
    ));
    let clients: String = wide
        .iter()
        .map(|c| format!("  {c}:\n    secret: {}\n", world::secret()))
        .collect();
    let mut crowd = String::from("visitors:\n");
    for i in 1..=VISITORS {
        crowd.push_str(&format!("  v{i:03}:\n    secret: {}\n", world::secret()));
    }
    let names: Vec<String> = (1..=VISITORS).map(|i| format!("v{i:03}")).collect();
    crowd.push_str(&format!(
        "services:\n  crowd:\n    client: home\n    target: 23\n    visitors: [{}]\n",
        names.join(", ")
    ));
    Options {
        extra_top: "web:\n  https: 127.0.0.1:HTTPS_PORT\n".into(),
        extra_services: services,
        extra_clients: clients,
        drop_ins: vec![("crowd.yaml", crowd, false)],
        ..Options::default()
    }
}

async fn count_is(c: &Ctx, css: &str, n: usize, what: &str) {
    c.ui.wait_true(
        &format!("document.querySelectorAll({css:?}).length === {n}"),
        what,
        SECS,
    )
    .await;
}

async fn show_all(c: &Ctx, at: &str, rows: &str, total: usize, what: &str) {
    let button = format!("{at} [data-testid=show-all]");
    count_is(
        c,
        &format!("{at} {rows}"),
        8,
        &format!("{what}: 8 rows at first"),
    )
    .await;
    let label = c.ui.text(&button).await;
    assert_eq!(label, format!("Show all {total}"), "{what}");
    c.ui.click(&button).await;
    count_is(
        c,
        &format!("{at} {rows}"),
        total,
        &format!("{what}: every row after Show all"),
    )
    .await;
    assert_eq!(c.ui.text(&button).await, "Show fewer", "{what}");
    c.ui.click(&button).await;
    count_is(c, &format!("{at} {rows}"), 8, &format!("{what}: back to 8")).await;
}

async fn big_list(c: &Ctx) {
    let started = Instant::now();
    c.ui.nav("/services").await;
    c.ui.wait_for(&format!("[data-row=s{SERVICES:03}]"), SECS)
        .await;
    println!(
        "scale: the services list showed its last of {} rows after {:?}",
        SERVICES + SITES + 4,
        started.elapsed()
    );
}

async fn served(c: &mut Ctx) {
    c.ui.nav("/clients/home").await;
    let at = "[data-testid=served]";
    c.ui.wait_for(&format!("{at} [data-row]"), SECS).await;
    assert_eq!(c.ui.count(&format!("{at} table")).await, 1);
    assert_eq!(
        c.ui.count("[data-testid=served-cards]").await,
        0,
        "only the table is built on a wide screen"
    );
    show_all(c, at, "[data-row]", SERVED, "services it serves").await;
    c.ui.view_shot("served-capped").await;
    c.ui.viewport(390, 844).await;
    c.ui.wait_for("[data-testid=served-cards]", SECS).await;
    assert_eq!(
        c.ui.count(&format!("{at} table")).await,
        0,
        "only the cards are built on a phone"
    );
    count_is(c, &format!("{at} [data-row]"), 8, "8 cards on a phone").await;
    c.ui.click(&format!("{at} [data-testid=show-all]")).await;
    count_is(
        c,
        &format!("{at} [data-row]"),
        SERVED,
        "every card after Show all",
    )
    .await;
    c.ui.click(&format!("{at} [data-testid=show-all]")).await;
    count_is(c, &format!("{at} [data-row]"), 8, "back to 8 cards").await;
    let _: bool = c
        .ui
        .eval("(() => { document.querySelector('[data-testid=served] [data-testid=show-all]').scrollIntoView({ block: 'center' }); return true; })()")
        .await;
    c.ui.view_shot("served-capped-phone").await;
    c.ui.viewport(1440, 900).await;
}

async fn health_and_menus(c: &mut Ctx) {
    c.ui.nav("/services/wide").await;
    let at = "[data-testid=health]";
    c.ui.wait_for(&format!("{at} tbody tr"), SECS).await;
    show_all(c, at, "tbody tr", WIDE, "health per client").await;
    c.ui.view_shot("health-capped").await;
    c.ui.nav("/services/wide?tab=settings").await;
    edit_mode(c).await;
    let entry = "[data-schema='services/*/clients'] [data-entry='0']";
    c.ui.wait_for(&format!("{entry} [data-testid=row-more]"), SECS)
        .await;
    assert_eq!(
        c.ui.count("[data-testid=row-menu]").await,
        0,
        "no row menu is built while all are closed"
    );
    c.ui.click(&format!("{entry} [data-testid=row-more]")).await;
    count_is(
        c,
        "[data-testid=row-menu]",
        1,
        "one menu is built once opened",
    )
    .await;
    c.ui.press(&format!("{entry} [data-testid=row-more]"), "Escape")
        .await;
    count_is(c, "[data-testid=row-menu]", 0, "the closed menu is gone").await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

async fn crowd(c: &mut Ctx) {
    c.ui.nav("/services/crowd?tab=settings").await;
    let view = "[data-testid=reached-by-view]";
    let text = c.ui.wait_text(view, "v200", SECS).await;
    assert!(text.contains("v001"), "{text}");
    assert_eq!(
        c.ui.count("[data-testid=reached-by] a, [data-testid=reached-by] input")
            .await,
        0,
        "view mode builds no editor and no link per visitor"
    );
    edit_mode(c).await;
    c.ui.wait_for("[data-way=visitors] #f-services-crowd-visitors", SECS)
        .await;
    assert_eq!(c.ui.count(view).await, 0, "edit mode drops the view lines");
    assert_eq!(
        c.ui.count("#f-services-crowd-bind").await,
        0,
        "a way that is off builds no field"
    );
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

fn scale(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.viewport(1440, 900).await;
        big_list(c).await;
        served(c).await;
        health_and_menus(c).await;
        crowd(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn long_lists_show_the_first_rows_and_show_all_on_request() {
    run("scale", opts(), false, scale);
}
