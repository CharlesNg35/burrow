use super::edits::{Flow, click_js};
use super::world::{self, Options};
use super::{
    Ctx, SECS, applied, both_widths, edit_mode, open, open_review, overview_tab, run, wait_checked,
};

const WEB_TOP: &str = "web:\n  https: 127.0.0.1:HTTPS_PORT\n";
const TABS: &str = "[data-testid=settings-tabs]";
const NAMES: &str = "f-web-redirect-names";

fn plain_web() -> Options {
    Options {
        extra_top: "web:\n  http: 127.0.0.1:HTTP_PORT\n".into(),
        ..Options::default()
    }
}

fn opts() -> Options {
    Options {
        extra_top: WEB_TOP.into(),
        ..Options::default()
    }
}

async fn every_width(c: &mut Ctx, step: &str) {
    for (w, name) in [(360, "360"), (390, "390"), (768, "tablet")] {
        c.ui.viewport(w, 900).await;
        let wide: bool = c
            .ui
            .eval("document.documentElement.scrollWidth > document.documentElement.clientWidth")
            .await;
        assert!(!wide, "{step} at {w}px scrolls sideways");
        c.ui.shot(&format!("{step}-{name}")).await;
    }
    c.ui.viewport(1440, 900).await;
}

async fn pick(c: &Ctx, tab: &str) {
    c.ui.click(&format!("{TABS} [data-tab={tab}]")).await;
    c.ui.wait_for(
        &format!("{TABS} [data-tab={tab}][aria-selected=true]"),
        SECS,
    )
    .await;
}

async fn the_four_tabs(c: &mut Ctx) {
    c.ui.nav("/settings").await;
    c.ui.wait_for(
        &format!("{TABS} [data-tab=server][aria-selected=true]"),
        SECS,
    )
    .await;
    assert_eq!(
        c.ui.all_attrs(&format!("{TABS} [role=tab]"), "data-tab")
            .await,
        ["server", "connections", "proxy", "monitoring"]
    );
    c.ui.wait_for("[data-testid=settings] [data-schema=listen]", SECS)
        .await;
    assert_eq!(
        c.ui.count("[data-testid=settings] [data-card=web]").await,
        0
    );
    both_widths(c, "settings-server").await;
    pick(c, "connections").await;
    c.ui.wait_for("[data-testid=settings] [data-card=limits]", SECS)
        .await;
    both_widths(c, "settings-connections").await;
    every_width(c, "settings-connections").await;
    pick(c, "proxy").await;
    c.ui.wait_for("[data-testid=settings] [data-card=web]", SECS)
        .await;
    let ca =
        c.ui.wait_text(
            "[data-testid=local-ca-pending]",
            "No local CA exists yet",
            SECS,
        )
        .await;
    assert!(
        ca.contains("HTTPS: Local CA") && !ca.contains("tls:"),
        "{ca}"
    );
    both_widths(c, "settings-web").await;
    pick(c, "monitoring").await;
    both_widths(c, "settings-monitoring").await;
    every_width(c, "settings-monitoring").await;
}

async fn search_finds_every_tab(c: &mut Ctx) {
    pick(c, "server").await;
    c.ui.fill("#list-search", "handshake").await;
    c.ui.wait_for(
        &format!("{TABS} [data-tab=connections][aria-selected=true]"),
        SECS,
    )
    .await;
    let pill = c.ui.text(&format!("{TABS} [data-tab=connections]")).await;
    assert!(pill.contains('1'), "the pill counts the match: {pill}");
    assert_eq!(
        c.ui.count("[data-testid=settings] [data-card]").await,
        1,
        "only the matching card shows"
    );
    both_widths(c, "settings-search").await;
    c.ui.fill("#list-search", "").await;
    c.ui.nav("/").await;
    c.ui.nav("/settings?card=web").await;
    c.ui.wait_for(
        &format!("{TABS} [data-tab=proxy][aria-selected=true]"),
        SECS,
    )
    .await;
}

async fn a_change_shows_on_its_pill(c: &mut Ctx) {
    c.ui.nav("/settings?tab=connections").await;
    edit_mode(c).await;
    let more = "[data-card='network.pool'] [data-testid=more-settings]";
    c.ui.click(more).await;
    c.ui.fill("#f-pool-max-idle", "32").await;
    wait_checked(c).await;
    c.ui.click("[data-testid=save]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
    let changed = format!("{TABS} [data-tab=connections] [data-tab-mark=changed]");
    let n = c.ui.wait_text(&changed, "1", SECS).await;
    assert_eq!(n.trim(), "1", "a changed dot and its count");
    both_widths(c, "settings-changed-pill").await;
}

fn tabs(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        the_four_tabs(c).await;
        search_finds_every_tab(c).await;
        a_change_shows_on_its_pill(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn settings_has_four_tabs_that_search_and_links_reach() {
    run("settings-tabs", opts(), false, tabs);
}

async fn add_a_redirect(c: &mut Ctx) {
    c.ui.nav("/settings?tab=proxy").await;
    edit_mode(c).await;
    c.ui.wait_for(&format!("#{NAMES}-add"), SECS).await;
    assert_eq!(
        c.ui.count("[data-card=web] [data-schema='web/redirect_names']")
            .await,
        1,
        "Names that only redirect is an essential field"
    );
    c.ui.fill(&format!("#{NAMES}-add"), "old.example.com").await;
    c.ui.click(&format!("#{NAMES}-add-button")).await;
    let text = format!("#{NAMES}-old-example-com-text");
    c.ui.wait_for(&text, SECS).await;
    c.ui.fill(&text, "https://example.com").await;
    wait_checked(c).await;
    both_widths(c, "redirect-added").await;
    open_review(c, "redirect-review").await;
    let review = c.ui.text("[data-testid=review]").await;
    assert!(review.contains("old.example.com"), "{review}");
    applied(c, "redirect-applied").await;
    let text = c.world.config();
    assert!(
        text.contains("old.example.com: https://example.com"),
        "{text}"
    );
}

async fn make_it_permanent(c: &mut Ctx) {
    c.ui.nav("/settings?tab=proxy").await;
    edit_mode(c).await;
    let text = format!("#{NAMES}-old-example-com-text");
    c.ui.wait_for(&text, SECS).await;
    c.ui.fill(&text, "https://example.net").await;
    click_js(c, &format!("#{NAMES}-old-example-com-permanent")).await;
    wait_checked(c).await;
    both_widths(c, "redirect-permanent").await;
    open_review(c, "redirect-permanent-review").await;
    applied(c, "redirect-permanent-applied").await;
    let text = c.world.config();
    assert!(
        text.contains("https://example.net") && text.contains("permanent: true"),
        "{text}"
    );
    c.ui.nav("/").await;
    overview_tab(c, "websites").await;
    let row =
        c.ui.wait_text("[data-web-row='old.example.com']", "example.net", SECS)
            .await;
    assert!(row.contains("permanent"), "the summary line says so: {row}");
    both_widths(c, "redirect-on-the-overview").await;
}

async fn remove_it(c: &mut Ctx) {
    c.ui.nav("/settings?tab=proxy").await;
    edit_mode(c).await;
    let remove = "[data-schema='web/redirect_names'] [data-key='old.example.com'] button";
    c.ui.wait_for(remove, SECS).await;
    c.ui.click(remove).await;
    wait_checked(c).await;
    open_review(c, "redirect-removed-review").await;
    applied(c, "redirect-removed").await;
    assert!(!c.world.config().contains("old.example.com"));
}

fn redirect_names(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        add_a_redirect(c).await;
        make_it_permanent(c).await;
        remove_it(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn names_that_only_redirect_are_added_edited_and_removed_in_settings() {
    run("redirect-names", plain_web(), false, redirect_names);
}

async fn turn_websocket_on(c: &mut Ctx, port: u16) {
    c.ui.nav("/settings?tab=connections").await;
    edit_mode(c).await;
    c.ui.wait_for("[data-card=websocket] [data-testid=not-set]", SECS)
        .await;
    click_js(c, "#f-websocket").await;
    c.ui.wait_for("#f-websocket-listen", SECS).await;
    c.ui.fill("#f-websocket-listen", &format!("127.0.0.1:{port}"))
        .await;
    wait_checked(c).await;
    both_widths(c, "websocket-on").await;
    open_review(c, "websocket-on-review").await;
    applied(c, "websocket-on-applied").await;
    let text = c.world.config();
    assert!(
        text.contains("websocket:") && text.contains(&format!("127.0.0.1:{port}")),
        "{text}"
    );
}

async fn turn_websocket_off(c: &mut Ctx) {
    c.ui.nav("/settings?tab=connections").await;
    edit_mode(c).await;
    c.ui.wait_for("#f-websocket-listen", SECS).await;
    click_js(c, "#f-websocket").await;
    c.ui.wait_for("[data-card=websocket] [data-testid=not-set]", SECS)
        .await;
    wait_checked(c).await;
    open_review(c, "websocket-off-review").await;
    let impact =
        c.ui.wait_text("[data-testid=impacts]", "turns WebSocket off", SECS)
            .await;
    assert!(
        impact.contains("no client is connected over WebSocket"),
        "{impact}"
    );
    c.ui.fill("[data-confirm=websocket]", "websocket").await;
    applied(c, "websocket-off-applied").await;
    assert!(!c.world.config().contains("websocket:"));
}

fn websocket(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        let port = world::free_port();
        turn_websocket_on(c, port).await;
        turn_websocket_off(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn websocket_is_turned_on_and_off_in_settings() {
    run("websocket", Options::default(), false, websocket);
}

const NOTIFY_TOP: &str = "notify:\n  - ntfy: https://ntfy.sh/burrow-e2e-topic\n    events: [client_offline, client_back]\n  - email:\n      to: ops@example.com\n      smtp: \"smtp://alerts@example.com@mail.example.com:587\"\n      password: ${SMTP_PASS}\n";

async fn notify_card(c: &mut Ctx) {
    c.ui.nav("/settings?tab=monitoring").await;
    c.ui.wait_for("[data-testid=settings] [data-card=monitoring]", SECS)
        .await;
    let card = c.ui.text("[data-card=monitoring]").await;
    assert!(card.contains("ntfy.sh/burrow-e2e-topic"), "{card}");
    assert!(
        card.contains("SMTP_PASS"),
        "the password shows by name: {card}"
    );
    assert!(!card.contains("e2e-smtp-value"), "{card}");
    let events =
        c.ui.all_attrs(
            "[data-card=monitoring] [data-testid=enum-list] > span",
            "data-tone",
        )
        .await;
    assert_eq!(events.len(), 2, "each event is a chip");
    let shown =
        c.ui.text("[data-card=monitoring] [data-testid=enum-list]")
            .await;
    assert!(
        shown.contains("Client offline") && shown.contains("Client back"),
        "{shown}"
    );
    both_widths(c, "settings-notify").await;
}

async fn notify_edit(c: &mut Ctx) {
    edit_mode(c).await;
    c.ui.wait_for("[data-item='0'] [data-testid=kind-choice]", SECS)
        .await;
    let first = c.ui.text("[data-card=monitoring] [data-item='0']").await;
    assert!(first.contains("ntfy topic"), "{first}");
    for other in [
        "#f-notify-0-webhook",
        "#f-notify-0-email",
        "#f-notify-1-ntfy",
    ] {
        assert_eq!(c.ui.count(other).await, 0, "only its own kind: {other}");
    }
    assert_eq!(c.ui.count("#f-notify-0-ntfy").await, 1);
    assert!(
        first.contains("Client offline"),
        "events by their labels: {first}"
    );
    assert!(!first.contains("client_offline"), "{first}");
    let second = c.ui.text("[data-card=monitoring] [data-item='1']").await;
    assert!(second.contains("Mail server"), "{second}");
    assert!(!second.contains("e2e-smtp-value"), "{second}");
    both_widths(c, "settings-notify-edit").await;
    click_js(c, "[data-item='0'] input[value=webhook]").await;
    c.ui.wait_for("#f-notify-0-webhook", SECS).await;
    let first = c.ui.text("[data-card=monitoring] [data-item='0']").await;
    assert!(
        !first.contains("ntfy.sh/burrow-e2e-topic"),
        "switching kind clears the old one: {first}"
    );
    both_widths(c, "settings-notify-webhook").await;
}

fn notify(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        notify_card(c).await;
        notify_edit(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn notifications_show_in_monitoring_with_the_mail_password_by_name() {
    let opts = Options {
        extra_top: NOTIFY_TOP.into(),
        server_env: vec![("SMTP_PASS".into(), "e2e-smtp-value".into())],
        ..Options::default()
    };
    run("settings-notify", opts, false, notify);
}
