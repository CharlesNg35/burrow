use super::edits::{Flow, row_error};
use super::world::Options;
use super::{Ctx, SECS, both_widths, edit_mode, open, run};

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

async fn said_once(c: &Ctx, schema: &str) {
    let hidden: bool = c
        .ui
        .eval(&format!(
            "(() => {{ const e = document.querySelector(\"[data-schema='{schema}'] [data-testid=stored-as]\"); return !e || e.offsetParent === null; }})()"
        ))
        .await;
    assert!(
        hidden,
        "{schema}: no Saved as hint under a value with an error"
    );
    assert_eq!(
        c.ui.count("[data-testid=new-website-problem]").await,
        0,
        "{schema}: the error at the field is not repeated above the save bar"
    );
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
    c.ui.fill("#new-website-names", "80").await;
    c.ui.press("#new-website-names", "Enter").await;
    let said =
        c.ui.wait_text("[data-testid=names-problem]", "80:", SECS)
            .await;
    assert!(
        said.contains("not a host name"),
        "a bad name says why at the field: {said}"
    );
    c.ui.click("[data-chip='80'] button").await;
    c.ui.wait_gone("[data-testid=names-problem]", SECS).await;
    c.ui.fill("#new-website-names", "shop.example.com").await;
    c.ui.press("#new-website-names", "Enter").await;
    c.ui.wait_for("[data-chip='shop.example.com']", SECS).await;
    let schema = "services/*/target";
    for (field, v, step) in [
        (PORT, "70000", "website-port-above-65535"),
        (PORT, "abc", "website-port-text"),
    ] {
        refused(c, field, schema, v, step).await;
        said_once(c, schema).await;
    }
    c.ui.fill(PORT, "8081").await;
    refused(c, HOST, schema, "bad host!", "website-host-bad").await;
    c.ui.fill(HOST, "127.0.0.1").await;
    c.ui.wait_for("[data-testid=new-website-email]", SECS).await;
    c.ui.fill("[data-testid=new-website-email] input", "a@b")
        .await;
    let _: bool =
        c.ui.eval(
            "(() => { if (document.activeElement) document.activeElement.blur(); return true; })()",
        )
        .await;
    let err = row_error(c, "web/letsencrypt/email").await;
    assert!(err.contains("is not an email address"), "{err}");
    assert_eq!(c.ui.count("[data-testid=new-website-problem]").await, 0);
    assert!(!c.ui.enabled("[data-testid=create]").await);
    both_widths(c, "website-email-bad").await;
    c.ui.click("[data-testid=create-cancel]").await;
}

async fn service_page(c: &mut Ctx) {
    c.ui.nav("/services/web?tab=settings").await;
    edit_mode(c).await;
    refused_on_page(c).await;
}

async fn refused_on_page(c: &mut Ctx) {
    c.ui.fill("#f-services-web-target", "70000").await;
    let _: bool =
        c.ui.eval(
            "(() => { if (document.activeElement) document.activeElement.blur(); return true; })()",
        )
        .await;
    let err = row_error(c, "services/*/target").await;
    assert!(err.contains("70000"), "{err}");
    said_once(c, "services/*/target").await;
    both_widths(c, "service-port-above-65535").await;
}

fn wizards(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        new_client(c).await;
        new_website(c).await;
        service_page(c).await;
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
