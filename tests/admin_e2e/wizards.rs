use super::edits::{Flow, row_error};
use super::world::Options;
use super::{Ctx, SECS, both_widths, open, run};

const MAX: &str = "[data-testid=new-max] input";
const PORT: &str = "#f-services--new-target";
const HOST: &str = "#f-services--new-target-host";

async fn refused(c: &mut Ctx, field: &str, schema: &str, value: &str, step: &str) {
    c.ui.fill(field, value).await;
    let _: bool =
        c.ui.eval(
            "(() => { if (document.activeElement) document.activeElement.blur(); return true; })()",
        )
        .await;
    let err = row_error(c, schema).await;
    assert!(!err.trim().is_empty(), "{value}: no error at the field");
    assert!(
        !c.ui.enabled("[data-testid=create]").await,
        "{value}: Create stays off"
    );
    both_widths(c, step).await;
}

async fn new_client(c: &mut Ctx) {
    c.ui.nav("/clients").await;
    c.ui.click("[data-testid=new]").await;
    c.ui.wait_for("[data-testid=new-entity]", SECS).await;
    c.ui.fill("#new-name", "Bad Name!").await;
    assert!(!c.ui.enabled("[data-testid=create]").await);
    both_widths(c, "client-bad-name").await;
    c.ui.fill("#new-name", "lab2").await;
    let schema = "clients/*/max_connections";
    for (v, step) in [
        ("0", "client-max-zero"),
        ("abc", "client-max-text"),
        ("4294967296", "client-max-above-u32"),
    ] {
        refused(c, MAX, schema, v, step).await;
        assert!(
            c.ui.text("#new-name-msg").await.trim().is_empty(),
            "{v}: the name keeps no error of the maximum"
        );
    }
    c.ui.fill(MAX, "8").await;
    c.ui.wait_true(
        "!document.querySelector('[data-testid=create]').disabled",
        "a good value lets Create on",
        SECS,
    )
    .await;
    both_widths(c, "client-max-good").await;
    c.ui.click("[data-testid=create-cancel]").await;
}

async fn new_website(c: &mut Ctx) {
    c.ui.nav("/services").await;
    c.ui.click("[data-testid=new]").await;
    c.ui.wait_for("[data-testid=new-menu] [data-new=website]", SECS)
        .await;
    c.ui.click("[data-new=website]").await;
    c.ui.wait_for("[data-testid=new-website]", SECS).await;
    c.ui.fill("#new-website-names", "shop.example.com").await;
    c.ui.press("#new-website-names", "Enter").await;
    c.ui.wait_for("[data-chip='shop.example.com']", SECS).await;
    let schema = "services/*/target";
    for (field, v, step) in [
        (PORT, "70000", "website-port-above-65535"),
        (PORT, "abc", "website-port-text"),
    ] {
        refused(c, field, schema, v, step).await;
    }
    c.ui.fill(PORT, "8081").await;
    refused(c, HOST, schema, "bad host!", "website-host-bad").await;
    c.ui.fill(HOST, "127.0.0.1").await;
    c.ui.wait_for("[data-testid=new-website-email]", SECS).await;
    c.ui.fill("[data-testid=new-website-email] input", "a@b")
        .await;
    c.ui.wait_text(
        "[data-testid=new-website-problem]",
        "is not an email address",
        SECS,
    )
    .await;
    assert!(!c.ui.enabled("[data-testid=create]").await);
    both_widths(c, "website-email-bad").await;
    c.ui.click("[data-testid=create-cancel]").await;
}

fn wizards(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        new_client(c).await;
        new_website(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_wizards_refuse_bad_input_at_the_field() {
    let opts = Options {
        extra_top: "web:\n  https: 127.0.0.1:HTTPS_PORT\n".into(),
        ..Options::default()
    };
    run("wizards", opts, false, wizards);
}
