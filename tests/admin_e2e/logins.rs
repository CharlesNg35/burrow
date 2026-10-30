use super::edits::Flow;
use super::site_keys::{open_more, settings_of};
use super::world::Options;
use super::{Ctx, SECS, applied, both_widths, edit_mode, open, open_review, run, wait_checked};

const TOP: &str = "web:\n  http: 127.0.0.1:HTTP_PORT\n  forward_auth:\n    authelia: { service: authelia, path: /api/authz/forward-auth, copy_headers: [Remote-User] }\n";
const SITES: &str = "  authelia:\n    client: home\n    target: 9091\n    web: auth.example.com\n  wiki:\n    client: home\n    target: 8080\n    web: { hosts: [wiki.example.com], forward_auth: authelia }\n";
const LOGINS: &str = "[data-schema='web/forward_auth']";
const PICKER: &str = "[data-schema='services/*/web/forward_auth']";

fn opts() -> Options {
    Options {
        extra_top: TOP.into(),
        extra_services: SITES.into(),
        ..Options::default()
    }
}

async fn rename_the_login_service(c: &mut Ctx) {
    c.ui.nav("/settings?tab=proxy").await;
    c.ui.wait_for("[data-schema=web]", SECS).await;
    edit_mode(c).await;
    open_more(c, "[data-schema=web] [data-testid=more-settings]").await;
    let row = format!("{LOGINS} [data-key=authelia]");
    c.ui.wait_for(&row, SECS).await;
    both_widths(c, "login-services-list").await;
    c.ui.click(&format!("{row} [data-testid=rename-key]")).await;
    c.ui.wait_for("[data-testid=rename-entity] #rename-to", SECS)
        .await;
    c.ui.fill("#rename-to", "sso").await;
    c.ui.click("[data-testid=rename]:not([disabled])").await;
    c.ui.wait_for(&format!("{LOGINS} [data-key=sso]"), SECS)
        .await;
    assert_eq!(c.ui.count(&row).await, 0, "the old name is gone");
    both_widths(c, "login-service-renamed").await;
}

async fn the_site_picker_follows(c: &mut Ctx) {
    settings_of(c, "wiki").await;
    edit_mode(c).await;
    c.ui.wait_text(PICKER, "sso", SECS).await;
    let picker = c.ui.text(PICKER).await;
    assert!(!picker.contains("authelia"), "{picker}");
    both_widths(c, "site-login-picker").await;
    wait_checked(c).await;
    open_review(c, "login-rename-review").await;
    applied(c, "login-rename-applied").await;
    let text = c.world.config();
    assert!(text.contains("    sso: {"), "{text}");
    assert!(text.contains("forward_auth: sso }"), "{text}");
    assert!(!text.contains("authelia: {"), "{text}");
}

fn logins(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        rename_the_login_service(c).await;
        the_site_picker_follows(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn renaming_a_login_service_renames_it_on_its_sites() {
    run("logins", opts(), false, logins);
}
