use super::edits::Flow;
use super::site_keys::settings_of;
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

fn card(name: &str) -> String {
    format!("{LOGINS} [data-login={name}]")
}

async fn menu(c: &Ctx, name: &str, item: &str) {
    let at = card(name);
    c.ui.click(&format!("{at} [data-testid=row-more]")).await;
    c.ui.wait_for(&format!("{at} [data-testid=row-menu]:not(.hidden)"), SECS)
        .await;
    c.ui.click(&format!("{at} [data-testid=row-menu] [data-testid={item}]"))
        .await;
}

async fn the_cards_say_what_each_one_does(c: &mut Ctx) {
    c.ui.nav("/settings?tab=proxy").await;
    c.ui.wait_for(&card("authelia"), SECS).await;
    let text = c.ui.text(&card("authelia")).await;
    for want in [
        "Authelia on service authelia, path /api/authz/forward-auth",
        "Passes to the app: Remote-User",
        "Used by 1 site: wiki",
    ] {
        assert!(text.contains(want), "{want} in {text}");
    }
    assert_eq!(
        c.ui.count("[data-testid=row-more]").await,
        0,
        "view mode has no menu"
    );
    let starred: bool =
        c.ui.eval("document.querySelector('[data-testid=settings]').innerText.includes('.*')")
            .await;
    assert!(!starred, "no raw * schema path is shown");
    both_widths(c, "login-services-view").await;
}

async fn typing_survives_a_rebase(c: &mut Ctx) {
    edit_mode(c).await;
    menu(c, "authelia", "login-edit").await;
    c.ui.wait_for("[data-testid=login-dialog] #login-path", SECS)
        .await;
    c.ui.fill("#login-path", "/typed/by/hand").await;
    c.world
        .hand_edit(|t| t.replace("    target: 8080\n", "    target: 8081\n"));
    c.ui.wait_text(
        "[data-testid=toasts]",
        "Config changed",
        std::time::Duration::from_secs(20),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let path: String =
        c.ui.eval("document.getElementById('login-path').value")
            .await;
    assert_eq!(
        path, "/typed/by/hand",
        "the open dialog keeps what was typed when the clean draft is re-based"
    );
    c.ui.click("[data-testid=login-dialog] button[aria-label=Close]")
        .await;
    c.ui.wait_gone("[data-testid=login-dialog]", SECS).await;
    c.ui.click("[data-testid=discard-edit]").await;
}

async fn add_from_a_preset(c: &mut Ctx) {
    edit_mode(c).await;
    c.ui.click("[data-testid=login-add]").await;
    c.ui.wait_for("[data-testid=login-dialog] #login-kind", SECS)
        .await;
    c.ui.choose("login-kind", "authentik").await;
    c.ui.wait_true(
        "document.getElementById('login-path').value === '/outpost.goauthentik.io/auth/caddy' && document.getElementById('login-name').value === 'authentik'",
        "the authentik preset fills the name and path",
        SECS,
    )
    .await;
    let dialog = c.ui.text("[data-testid=login-dialog]").await;
    assert!(dialog.contains("X-Authentik-Username"), "{dialog}");
    c.ui.choose("login-kind", "Other").await;
    c.ui.wait_true(
        "document.getElementById('login-path').value === ''",
        "Other leaves the path empty",
        SECS,
    )
    .await;
    c.ui.choose("login-kind", "oauth2-proxy").await;
    c.ui.wait_text("[data-testid=login-dialog]", "X-Auth-Request-User", SECS)
        .await;
    c.ui.choose("login-service", "authelia").await;
    both_widths(c, "login-add-dialog").await;
    c.ui.click("[data-testid=login-save]:not([disabled])").await;
    c.ui.wait_for(&card("oauth2-proxy"), SECS).await;
    let text = c.ui.text(&card("oauth2-proxy")).await;
    assert!(text.contains("Not used by any site yet"), "{text}");
    assert!(
        text.contains("X-Auth-Request-User, X-Auth-Request-Email"),
        "{text}"
    );
    both_widths(c, "login-services-edit").await;
}

async fn remove_warns_and_rename_follows(c: &mut Ctx) {
    menu(c, "authelia", "login-remove").await;
    let warn =
        c.ui.wait_text("[data-testid=login-remove-dialog]", "wiki", SECS)
            .await;
    assert!(warn.contains("wiki uses it"), "{warn}");
    c.ui.shot("login-remove-warning").await;
    c.ui.click("[data-testid=login-remove-dialog] button:not([data-testid])")
        .await;
    c.ui.wait_gone("[data-testid=login-remove-dialog]", SECS)
        .await;
    menu(c, "oauth2-proxy", "login-remove").await;
    c.ui.click("[data-testid=login-remove-confirm]").await;
    c.ui.wait_gone(&card("oauth2-proxy"), SECS).await;
    menu(c, "authelia", "rename-key").await;
    c.ui.wait_for("[data-testid=rename-entity] #rename-to", SECS)
        .await;
    c.ui.fill("#rename-to", "sso").await;
    c.ui.click("[data-testid=rename]:not([disabled])").await;
    c.ui.wait_for(&card("sso"), SECS).await;
    assert_eq!(
        c.ui.count(&card("authelia")).await,
        0,
        "the old name is gone"
    );
    both_widths(c, "login-service-renamed").await;
}

async fn the_site_picks_one(c: &mut Ctx) {
    settings_of(c, "wiki").await;
    edit_mode(c).await;
    c.ui.wait_for(&format!("{PICKER} [data-testid=site-login]"), SECS)
        .await;
    let note =
        c.ui.text(&format!("{PICKER} [data-testid=login-note]"))
            .await;
    assert!(note.contains("the app gets Remote-User"), "{note}");
    assert!(
        c.ui.count(&format!("{PICKER} [data-testid=manage-logins]"))
            .await
            == 1,
        "the picker links to the login services"
    );
    let checked: String =
        c.ui.eval(&format!(
            "(document.querySelector(\"{PICKER} input[type=radio]:checked\") || {{}}).value || ''"
        ))
        .await;
    assert_eq!(checked, "sso", "None | sso, with the renamed one chosen");
    both_widths(c, "site-login-picker").await;
    wait_checked(c).await;
    open_review(c, "login-rename-review").await;
    applied(c, "login-rename-applied").await;
    let text = c.world.config();
    assert!(text.contains("    sso: {"), "{text}");
    assert!(text.contains("forward_auth: sso }"), "{text}");
    assert!(!text.contains("authelia: {"), "{text}");
    assert!(!text.contains("oauth2-proxy"), "{text}");
}

fn logins(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        the_cards_say_what_each_one_does(c).await;
        typing_survives_a_rebase(c).await;
        add_from_a_preset(c).await;
        remove_warns_and_rename_follows(c).await;
        the_site_picks_one(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn login_services_are_cards_added_from_presets_and_picked_by_sites() {
    run("logins", opts(), false, logins);
}
