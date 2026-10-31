use std::path::PathBuf;

use super::edits::Flow;
use super::world::{Options, free_port};
use super::{Ctx, SECS, applied, apply_now, chrome_gate, edit_mode, gate, open, open_review};

const ENTRIES: &str = "  pool:\n    clients: [home, { name: home, target: 127.0.0.1:9 }]\n    target: 22\n    visitors: [laptop]\n";
const NOTICE: &str = "older than the server";
const OLD_APPLIED: &str = "Applied rev";

fn older_admin(entries: bool) -> Option<Options> {
    let old = super::older::older()?;
    Some(Options {
        admin_bin: Some(old),
        extra_services: if entries {
            ENTRIES.into()
        } else {
            String::new()
        },
        ..Options::default()
    })
}

async fn no_banner(c: &Ctx, screen: &str) {
    assert_eq!(
        c.ui.count("[data-testid^=banner-]").await,
        0,
        "no error banner on {screen}"
    );
}

async fn dashboard(c: &mut Ctx, step: &str) -> String {
    open(c, "ops").await;
    c.ui.wait_for("[data-testid=overview]", SECS).await;
    c.ui.wait_for("[data-testid=attention] [data-testid=count]", SECS)
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    no_banner(c, "the dashboard").await;
    c.ui.shot(step).await;
    c.ui.text("[data-testid=attention-card]").await
}

async fn settings_of(c: &Ctx, service: &str) {
    c.ui.nav(&format!("/services/{service}")).await;
    c.ui.click("[role=tablist] [data-tab=settings]").await;
}

async fn change_bind(c: &mut Ctx) -> u16 {
    let port = free_port();
    settings_of(c, "web").await;
    c.ui.wait_for("[data-schema='services/*/bind']", SECS).await;
    no_banner(c, "the service page").await;
    edit_mode(c).await;
    c.ui.fill("#f-services-web-bind", &format!("127.0.0.1:{port}"))
        .await;
    open_review(c, "review-bind").await;
    port
}

fn bound_on(c: &Ctx, port: u16) {
    let text = c.world.config();
    assert!(
        text.contains(&format!("bind: 127.0.0.1:{port}")),
        "the server wrote the new bind: {text}"
    );
    assert!(
        text.contains("# keep this comment"),
        "comments survive: {text}"
    );
}

fn an_older_admin_on_a_config_it_knows(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let attention = dashboard(c, "older-admin-dashboard").await;
        assert!(
            !attention.contains(NOTICE),
            "a config v0.1.2 can edit needs no notice: {attention}"
        );
        let port = change_bind(c).await;
        apply_now(c, OLD_APPLIED).await;
        c.ui.shot("older-admin-applied").await;
        bound_on(c, port);
    })
}

fn an_older_admin_on_client_entries(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let attention = dashboard(c, "older-admin-notice").await;
        assert!(
            attention.contains(NOTICE) && attention.contains("Clients of service pool"),
            "the older admin is told what it cannot edit: {attention}"
        );
        settings_of(c, "pool").await;
        let clients = "[data-schema='services/*/clients']";
        c.ui.wait_for(clients, SECS).await;
        no_banner(c, "a service with client entries").await;
        let raw =
            c.ui.wait_text(
                &format!("{clients} [data-testid=raw-node]"),
                "needs a newer burrow admin",
                SECS,
            )
            .await;
        assert!(
            raw.contains("127.0.0.1:9"),
            "the entries are shown as they are: {raw}"
        );
        c.ui.shot("older-admin-raw-clients").await;
        let port = change_bind(c).await;
        apply_now(c, OLD_APPLIED).await;
        bound_on(c, port);
        let text = c.world.config();
        assert!(
            text.contains("{ name: home, target: 127.0.0.1:9 }"),
            "an edit elsewhere keeps the client entries: {text}"
        );
    })
}

fn the_new_admin_on_an_older_server(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        dashboard(c, "older-server-dashboard").await;
        let port = change_bind(c).await;
        applied(c, "older-server-applied").await;
        bound_on(c, port);
        c.ui.assert_no_errors();
    })
}

#[test]
fn compat_an_older_admin_edits_a_config_it_knows_with_no_notice() {
    let Some(opts) = older_admin(false) else {
        return;
    };
    super::run_gated(
        "compat-older-admin",
        chrome_gate(false),
        opts,
        false,
        an_older_admin_on_a_config_it_knows,
    );
}

#[test]
fn compat_an_older_admin_is_told_it_cannot_edit_client_entries_and_still_edits() {
    let Some(opts) = older_admin(true) else {
        return;
    };
    super::run_gated(
        "compat-older-admin-entries",
        chrome_gate(false),
        opts,
        false,
        an_older_admin_on_client_entries,
    );
}

#[test]
fn compat_the_new_admin_edits_an_older_server() {
    let Some(old): Option<PathBuf> = super::older::older() else {
        return;
    };
    let opts = Options {
        server_bin: Some(old),
        ..Options::default()
    };
    super::run_gated(
        "compat-older-server",
        gate(),
        opts,
        false,
        the_new_admin_on_an_older_server,
    );
}
