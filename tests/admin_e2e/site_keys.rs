use super::edits::Flow;
use super::world::Options;
use super::{
    Ctx, SECS, applied, both_widths, edit_mode, open, open_review, overview_tab, run, wait_checked,
};

const TOP: &str = "web:\n  https: 127.0.0.1:HTTPS_PORT\n  http: 127.0.0.1:HTTP_PORT\n  trusted_proxies: [127.0.0.1/32]\n  letsencrypt:\n    email: ops@example.com\n    directory: https://127.0.0.1:1/directory\n    dns:\n      - domains: [example.org]\n        cloudflare: { token: \"${CF_TOKEN}\" }\n";
const SITES: &str = "  site:\n    client: home\n    target: 8080\n    web: { hosts: [example.com], tls: off }\n  api:\n    client: home\n    target: 9000\n    web: { hosts: [example.com], tls: off, paths: [/api], app_path: / }\n";
const WEBSITE_MORE: &str = "[data-more$='.website'] [data-testid=more-settings]";

fn opts() -> Options {
    Options {
        extra_top: TOP.into(),
        extra_services: SITES.into(),
        server_env: vec![("CF_TOKEN".into(), "cf-token-for-the-e2e".into())],
        ..Options::default()
    }
}

pub(super) async fn open_more(c: &Ctx, css: &str) {
    let _: bool = c
        .ui
        .eval(&format!(
            "(() => {{ document.querySelectorAll({}).forEach(b => {{ if (b.getAttribute('aria-expanded') !== 'true') b.click(); }}); return true; }})()",
            serde_json::to_string(css).expect("json")
        ))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
}

pub(super) async fn settings_of(c: &Ctx, service: &str) {
    c.ui.nav(&format!("/services/{service}")).await;
    c.ui.wait_for("[role=tablist]", SECS).await;
    c.ui.click("[role=tablist] [data-tab=settings]").await;
    c.ui.wait_for("[data-form-tab]", SECS).await;
}

async fn routes_on_the_overview(c: &mut Ctx) {
    overview_tab(c, "websites").await;
    c.ui.wait_for("[data-web-row='example.com'] [data-web-route='/api']", SECS)
        .await;
    let row = c.ui.text("[data-web-row='example.com']").await;
    assert!(row.contains("/api") && row.contains("api"), "{row}");
    assert!(row.contains("site"), "the rest of the name: {row}");
    both_widths(c, "overview-routes").await;
}

async fn a_header_and_a_path(c: &mut Ctx) {
    settings_of(c, "api").await;
    edit_mode(c).await;
    open_more(c, WEBSITE_MORE).await;
    c.ui.wait_for("#f-services-api-web-response-headers-add", SECS)
        .await;
    both_widths(c, "site-more-settings").await;
    let add = "#f-services-api-web-response-headers-add";
    c.ui.fill(add, "X-Frame-Options").await;
    c.ui.click("#f-services-api-web-response-headers-add-button")
        .await;
    let value = "#f-services-api-web-response-headers-X-Frame-Options";
    c.ui.wait_for(value, SECS).await;
    c.ui.fill(value, "DENY").await;
    super::edits::click_js(
        c,
        "[data-schema='services/*/web/target_tls'] input[value=own-ca]",
    )
    .await;
    c.ui.wait_for(
        "[data-schema='services/*/web/target_tls'] [data-testid=file-upload][data-kind=ca]",
        SECS,
    )
    .await;
    both_widths(c, "site-target-own-ca").await;
    super::edits::click_js(
        c,
        "[data-schema='services/*/web/target_tls'] input[value=unchecked]",
    )
    .await;
    wait_checked(c).await;
    both_widths(c, "site-header-added").await;
    open_review(c, "site-header-review").await;
    applied(c, "site-header-applied").await;
    let text = c.world.config();
    assert!(text.contains("X-Frame-Options: DENY"), "{text}");
    assert!(text.contains("insecure_skip_verify: true"), "{text}");
}

async fn dns_secrets_are_references(c: &mut Ctx) {
    c.ui.nav("/settings?tab=proxy").await;
    c.ui.wait_for("[data-schema=web]", SECS).await;
    open_more(c, "[data-schema=web] [data-testid=more-settings]").await;
    c.ui.wait_text("[data-schema=web]", "set, from ${CF_TOKEN}", SECS)
        .await;
    let page = c.ui.text("[data-schema=web]").await;
    assert!(
        !page.contains("cf-token-for-the-e2e"),
        "the token never shows: {page}"
    );
    both_widths(c, "settings-dns-provider").await;
    edit_mode(c).await;
    let token = "[data-schema='web/letsencrypt/dns/*/cloudflare/token']";
    c.ui.wait_text(token, "Change this in server.yaml", SECS)
        .await;
    assert_eq!(
        c.ui.count(&format!("{token} input")).await,
        0,
        "a DNS secret reference is changed in server.yaml only"
    );
    let api = c.ui.text(token).await;
    assert!(api.contains("${CF_TOKEN}") || api.contains("set"), "{api}");
    both_widths(c, "settings-dns-token-locked").await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

fn site_keys(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        routes_on_the_overview(c).await;
        a_header_and_a_path(c).await;
        dns_secrets_are_references(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_site_keys_routes_and_dns_secrets_show_in_the_admin_ui() {
    run("site-keys", opts(), false, site_keys);
}
