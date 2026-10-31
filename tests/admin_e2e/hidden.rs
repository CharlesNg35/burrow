use super::edits::Flow;
use super::world::Options;
use super::{Ctx, SECS, applied, edit_mode, open, open_more, open_review, run, wait_checked};

const TOP: &str = "web:\n  http: 127.0.0.1:HTTP_PORT\n";
const SITE: &str = "  site:\n    client: home\n    target: 8080\n    web:\n      hosts: [example.com]\n      tls: off\n      request_headers: { Authorization: \"Bearer e2e-header-credential\", X-Plain: hello, X-Debug: \"id SECRET\" }\n";
const HEADERS: &str = "[data-schema='services/*/web/request_headers']";

fn opts() -> Options {
    Options {
        extra_top: TOP.into(),
        extra_services: SITE.replace("SECRET", &burrow::wire::secret::Secret::generate().encode()),
        ..Options::default()
    }
}

async fn row_text(c: &Ctx, key: &str) -> String {
    c.ui.eval(&format!(
        "(() => {{ const r = document.querySelector(\"{HEADERS} [data-key='{key}']\"); if (!r) return ''; return r.textContent + ' ' + [...r.querySelectorAll('input')].map(i => i.value).join(' '); }})()"
    ))
    .await
}

async fn at_widths(c: &mut Ctx, step: &str) {
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    c.ui.shot(step).await;
    c.ui.viewport(390, 900).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    c.ui.shot(&format!("{step}-390")).await;
    c.ui.viewport(1440, 900).await;
}

async fn edit_headers(c: &mut Ctx) {
    super::site_keys::settings_of(c, "site").await;
    edit_mode(c).await;
    open_more(c, "services/*/web/request_headers").await;
    c.ui.wait_for(&format!("{HEADERS} [data-key=Authorization]"), SECS)
        .await;
}

async fn masked_in_the_form(c: &mut Ctx) {
    edit_headers(c).await;
    at_widths(c, "masked-header-edit").await;
    let row = row_text(c, "Authorization").await;
    assert!(
        !row.contains("$secret") && !row.contains("e2e-header-credential") && !row.contains('{'),
        "the masked value shows no marker and no value: {row}"
    );
    assert!(row.contains("hidden"), "{row}");
    let other = row_text(c, "X-Debug").await;
    assert!(
        !other.contains("$secret") && !other.contains("id ") && other.contains("hidden"),
        "a value that holds a burrow secret is hidden the same way: {other}"
    );
    assert_eq!(
        c.ui.count(&format!("{HEADERS} [data-key=Authorization] input"))
            .await,
        0,
        "a masked value is not an empty-looking text box"
    );
    c.ui.fill(
        "#f-services-site-web-request-headers-X-Plain",
        "hello again",
    )
    .await;
    wait_checked(c).await;
    open_review(c, "masked-header-other-edit-review").await;
    applied(c, "masked-header-other-edit-applied").await;
    let text = c.world.config();
    assert!(
        text.contains("Bearer e2e-header-credential")
            && text.contains("hello again")
            && text.contains("X-Debug: \"id "),
        "an untouched masked value is kept: {text}"
    );
}

async fn replaced_in_the_form(c: &mut Ctx) {
    edit_headers(c).await;
    let replace = format!("{HEADERS} [data-key=Authorization] [data-testid=replace-hidden]");
    c.ui.click(&replace).await;
    let input = "#f-services-site-web-request-headers-Authorization";
    c.ui.wait_for(input, SECS).await;
    assert_eq!(
        c.ui.attr_of(input, "value").await,
        "",
        "replacing starts from an empty box, never the old value"
    );
    c.ui.click(&format!(
        "{HEADERS} [data-key=Authorization] [data-testid=keep-hidden]"
    ))
    .await;
    c.ui.wait_for(&replace, SECS).await;
    assert_eq!(
        c.ui.count("[data-testid=save]:not([disabled])").await,
        0,
        "keeping the old value leaves nothing to save"
    );
    c.ui.click(&replace).await;
    c.ui.wait_for(input, SECS).await;
    c.ui.fill(input, "Bearer e2e-replaced").await;
    at_widths(c, "masked-header-replacing").await;
    wait_checked(c).await;
    open_review(c, "masked-header-replace-review").await;
    let review = c.ui.text("[data-testid=review]").await;
    assert!(
        !review.contains("e2e-replaced") && !review.contains("e2e-header-credential"),
        "{review}"
    );
    applied(c, "masked-header-replaced").await;
    let text = c.world.config();
    assert!(
        text.contains("Bearer e2e-replaced") && !text.contains("e2e-header-credential"),
        "{text}"
    );
}

fn masked(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        masked_in_the_form(c).await;
        replaced_in_the_form(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_masked_value_is_kept_untouched_and_replaced_from_an_empty_box() {
    run("masked-values", opts(), false, masked);
}
