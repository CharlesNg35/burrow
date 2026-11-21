use super::edits::{Flow, click_js, row_clean};
use super::world::Options;
use super::{Ctx, SECS, both_widths, edit_mode, open, run, wait_checked};

const WAND: &str = "[data-schema='services/*/bind'] [data-testid=free-port]";

async fn value(c: &Ctx, id: &str) -> String {
    c.ui.eval(&format!("document.querySelector('{id}').value"))
        .await
}

async fn proposed(c: &Ctx, before: &str) -> String {
    c.ui.click(WAND).await;
    c.ui.wait_true(
        &format!(
            "(() => {{ const v = document.querySelector('#f-services-web-bind').value; return /^[0-9]+$/.test(v) && v !== {before:?}; }})()"
        ),
        "the wand fills a new free port",
        SECS,
    )
    .await;
    value(c, "#f-services-web-bind").await
}

async fn the_wand_sits_on_the_field(c: &Ctx) {
    let off: f64 = c
        .ui
        .eval(&format!(
            "(() => {{ const b = document.querySelector(\"{WAND}\").getBoundingClientRect(); const i = document.querySelector('#f-services-web-bind').getBoundingClientRect(); return Math.abs((b.top + b.height / 2) - (i.top + i.height / 2)) + Math.abs(b.height - i.height); }})()"
        ))
        .await;
    assert!(
        off <= 1.0,
        "the wand is the field's height, centred on it: {off}"
    );
}

async fn one_free_port(c: &mut Ctx) {
    c.ui.nav("/services/web").await;
    edit_mode(c).await;
    assert_eq!(c.ui.count(WAND).await, 1, "the public port has the wand");
    assert_eq!(
        c.ui.count("[data-schema='services/*/target'] [data-testid=free-port]")
            .await,
        0,
        "the target has none"
    );
    let label: String =
        c.ui.eval(&format!(
            "document.querySelector(\"{WAND}\").getAttribute('aria-label')"
        ))
        .await;
    assert_eq!(label, "Propose a free port");
    the_wand_sits_on_the_field(c).await;
    c.ui.fill("#f-services-web-bind", "").await;
    let first = proposed(c, "").await;
    wait_checked(c).await;
    row_clean(c, "services/*/bind").await;
    let next = proposed(c, &first).await;
    assert!(
        next.parse::<u32>().unwrap() > first.parse::<u32>().unwrap(),
        "another click gives the next free port: {first} then {next}"
    );
    wait_checked(c).await;
    row_clean(c, "services/*/bind").await;
    both_widths(c, "free-port-single").await;
}

async fn a_free_block_for_a_range(c: &mut Ctx) {
    let echo = c.world.ports["echo"];
    c.ui.fill(
        "#f-services-web-target",
        &format!("127.0.0.1:{echo}-{}", echo + 2),
    )
    .await;
    click_js(c, "#f-services-web-bind-range").await;
    c.ui.wait_true(
        &format!("document.querySelector(\"{WAND}\").getAttribute('aria-label') === 'Propose 3 free ports in a row'"),
        "a range proposes the size of the target's range",
        SECS,
    )
    .await;
    c.ui.click(WAND).await;
    c.ui.wait_true(
        "(() => { const a = +document.querySelector('#f-services-web-bind').value, b = +document.querySelector('#f-services-web-bind-to').value; return a > 0 && b === a + 2; })()",
        "the wand fills a block of 3",
        SECS,
    )
    .await;
    wait_checked(c).await;
    row_clean(c, "services/*/bind").await;
    the_wand_sits_on_the_field(c).await;
    both_widths(c, "free-port-range").await;
}

fn free_port(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        one_free_port(c).await;
        a_free_block_for_a_range(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_wand_proposes_a_free_public_port_or_block() {
    run("free-port", Options::default(), false, free_port);
}
