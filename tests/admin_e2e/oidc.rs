use super::edits::Flow;
use super::site_keys::settings_of;
use super::world::Options;
use super::{Ctx, SECS, applied, both_widths, edit_mode, open, open_review, run, wait_checked};

const TOP: &str = "web:\n  https: 127.0.0.1:HTTPS_PORT\n  http: 127.0.0.1:HTTP_PORT\n  oidc:\n    work: { issuer: \"https://sso.example.com/realms/home\", client_id: burrow, client_secret: \"${WORK_SECRET}\" }\n";
const SITES: &str =
    "  wiki:\n    client: home\n    target: 8080\n    web: { hosts: [wiki.lan], tls: internal }\n";
const PROVIDERS: &str = "[data-schema='web/oidc']";
const PICKER: &str = "[data-schema='services/*/web/oidc']";
const ALLOW: &str = "[data-schema='services/*/web/allow_users']";

fn opts() -> Options {
    Options {
        extra_top: TOP.into(),
        extra_services: SITES.into(),
        server_env: vec![("WORK_SECRET".into(), "work-value".into())],
        ..Options::default()
    }
}

fn card(name: &str) -> String {
    format!("{PROVIDERS} [data-login={name}]")
}

async fn the_card_shows_the_secret_by_name(c: &mut Ctx) {
    c.ui.nav("/settings?tab=proxy").await;
    c.ui.wait_for(&card("work"), SECS).await;
    let text = c.ui.text(&card("work")).await;
    for want in [
        "https://sso.example.com/realms/home",
        "Client ID burrow",
        "Client secret: from ${WORK_SECRET}",
        "Not used by any site yet",
    ] {
        assert!(text.contains(want), "{want} in {text}");
    }
    let page: String = c.ui.eval("document.body.innerText").await;
    assert!(!page.contains("work-value"), "never the secret's value");
    both_widths(c, "oidc-providers-view").await;
}

async fn add_a_provider(c: &mut Ctx) {
    edit_mode(c).await;
    c.ui.click("[data-testid=oidc-add]").await;
    c.ui.wait_for("[data-testid=oidc-dialog] #oidc-issuer", SECS)
        .await;
    c.ui.fill("#oidc-name", "google").await;
    c.ui.fill("#oidc-issuer", "accounts.google.com").await;
    c.ui.wait_text(
        "[data-testid=dialog-status]",
        "must start with https://",
        SECS,
    )
    .await;
    c.ui.fill("#oidc-issuer", "https://accounts.google.com")
        .await;
    c.ui.fill("#oidc-client-id", "1234.apps.googleusercontent.com")
        .await;
    let locked = c.ui.text("[data-testid=oidc-secret-locked]").await;
    assert!(
        locked.contains("client_secret: ${GOOGLE_SECRET} under web.oidc.google"),
        "the secret is written on the server, never through the admin: {locked}"
    );
    assert_eq!(c.ui.count("#oidc-secret").await, 0);
    let note = c.ui.text("[data-testid=oidc-callback-note]").await;
    assert!(note.contains("/.burrow/login/callback"), "{note}");
    both_widths(c, "oidc-add-dialog").await;
    c.ui.click("[data-testid=oidc-save]:not([disabled])").await;
    c.ui.wait_for(&card("google"), SECS).await;
    let text = c.ui.text(&card("google")).await;
    assert!(
        text.contains("Client secret: none (public client)"),
        "{text}"
    );
    both_widths(c, "oidc-providers-edit").await;
    c.ui.click("[data-testid=save-bar] [data-testid=save]")
        .await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
    wait_checked(c).await;
}

async fn the_site_picks_two_and_lists_its_users(c: &mut Ctx) {
    settings_of(c, "wiki").await;
    edit_mode(c).await;
    let input = format!("{PICKER} [data-testid=site-oidc] input[role=combobox]");
    c.ui.wait_for(&input, SECS).await;
    for name in ["google", "work"] {
        c.ui.click(&input).await;
        let option = format!("[role=listbox] [data-option={name}]");
        c.ui.wait_for(&option, SECS).await;
        c.ui.click(&option).await;
    }
    let note =
        c.ui.wait_text(
            &format!("{PICKER} [data-testid=oidc-note]"),
            "visitors pick one",
            SECS,
        )
        .await;
    assert!(
        note.contains("google (https://accounts.google.com)"),
        "{note}"
    );
    assert_eq!(
        c.ui.count(&format!("{PICKER} [data-testid=manage-oidc]"))
            .await,
        1,
        "the picker links to the login providers"
    );
    let allow = format!("{ALLOW} input");
    for user in ["alice@gmail.com", "@example.com"] {
        c.ui.fill(&allow, user).await;
        c.ui.press(&allow, "Enter").await;
        c.ui.wait_for(&format!("{ALLOW} [data-chip='{user}']"), SECS)
            .await;
    }
    both_widths(c, "site-oidc-picker").await;
    wait_checked(c).await;
    open_review(c, "oidc-review").await;
    applied(c, "oidc-applied").await;
    let text = c.world.config();
    assert!(
        text.contains("    google:\n      issuer: https://accounts.google.com\n      client_id: 1234.apps.googleusercontent.com\n"),
        "{text}"
    );
    assert!(text.contains("${WORK_SECRET}"), "the kept secret: {text}");
    assert!(
        text.contains("      oidc:\n        - google\n        - work\n      allow_users:\n        - alice@gmail.com\n        - \"@example.com\"\n"),
        "{text}"
    );
}

fn providers(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        the_card_shows_the_secret_by_name(c).await;
        add_a_provider(c).await;
        the_site_picks_two_and_lists_its_users(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn login_providers_are_cards_with_secrets_by_name_and_sites_pick_several() {
    run("oidc", opts(), false, providers);
}
