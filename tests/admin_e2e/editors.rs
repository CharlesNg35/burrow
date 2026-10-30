use std::time::Duration;

use super::edits::open_more;
use super::edits::{Flow, clear_of_bar, click_js, focus, row_clean, row_error, stored_as};
use super::world::{Options, free_range};
use super::{Ctx, SECS, applied, edit_mode, open, open_review, run, wait_checked};

fn editors(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        let public = free_range(2);
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        edit_mode(c).await;

        let bind = "services/*/bind";
        c.ui.fill("#f-services-web-bind", "70000").await;
        let err = row_error(c, bind).await;
        assert!(!err.is_empty(), "port 70000 is refused inline");
        c.ui.shot("bind-port-out-of-range").await;
        c.ui.fill("#f-services-web-bind", &format!("127.0.0.1:{public}"))
            .await;
        row_clean(c, bind).await;
        assert_eq!(
            c.ui.count("[data-schema='services/*/bind'] [data-testid=port-count]")
                .await,
            0,
            "one port needs no count beside it"
        );
        c.ui.select("#f-services-web-bind-host", "\u{1}specific")
            .await;
        c.ui.fill("#f-services-web-bind-host-ip", "999.1.1.1").await;
        let err = row_error(c, bind).await;
        assert!(!err.is_empty(), "a bad IP is refused inline");
        c.ui.shot("bind-bad-ip").await;
        c.ui.select("#f-services-web-bind-host", "127.0.0.1").await;
        row_clean(c, bind).await;

        let target = "services/*/target";
        click_js(c, "[data-schema='services/*/target'] input[value=unix]").await;
        c.ui.fill("#f-services-web-target", "/tmp/burrow-e2e.sock")
            .await;
        stored_as(c, "services/*/target", "unix:/tmp/burrow-e2e.sock").await;
        c.ui.shot("target-unix-socket").await;
        click_js(c, "[data-schema='services/*/target'] input[value=network]").await;
        c.ui.fill("#f-services-web-target-host", "127.0.0.1").await;
        c.ui.fill("#f-services-web-target", "abc").await;
        let err = row_error(c, target).await;
        assert!(!err.is_empty(), "a bad target port is refused inline");
        c.ui.fill("#f-services-web-target", &c.world.ports["echo"].to_string())
            .await;
        row_clean(c, target).await;

        open_more(c, "services/*/health_check").await;
        click_js(
            c,
            "[data-schema='services/*/health_check'] input[value=custom]",
        )
        .await;
        c.ui.wait_for("[data-testid=custom-box]", SECS).await;
        c.ui.fill("#f-services-web-health-check-interval-secs", "0")
            .await;
        let err = row_error(c, "services/*/health_check/interval_secs").await;
        assert!(!err.is_empty(), "an interval of 0 is refused inline");
        c.ui.shot("health-check-custom-invalid").await;
        c.ui.fill("#f-services-web-health-check-interval-secs", "5")
            .await;
        row_clean(c, "services/*/health_check/interval_secs").await;
        c.ui.shot("health-check-custom").await;

        let client = "[data-schema='services/*/clients']";
        c.ui.click(&format!("{client} [data-entry='0'] [data-testid=row-more]"))
            .await;
        c.ui.click(&format!(
            "{client} [data-entry='0'] [data-testid=entry-remove]"
        ))
        .await;
        c.ui.wait_for("[data-testid=form-problems]", SECS).await;
        c.ui.shot("no-client-is-a-problem").await;
        c.ui.fill("#f-services-web-clients", "off").await;
        focus(c, "f-services-web-clients").await;
        assert_eq!(
            c.ui.count(&format!("{client} [data-option]")).await,
            1,
            "typing filters the clients"
        );
        c.ui.shot("picker-filtered").await;
        c.ui.fill("#f-services-web-clients", "ho").await;
        focus(c, "f-services-web-clients").await;
        c.ui.press("#f-services-web-clients", "Enter").await;
        c.ui.wait_for(&format!("{client} [data-chip=home]"), SECS)
            .await;
        wait_checked(c).await;
        c.ui.click("[data-testid=save]").await;
        open_review(c, "review-of-the-editors").await;
        applied(c, "editors-applied").await;
        let text = c.world.config();
        assert!(
            text.contains(&format!("bind: 127.0.0.1:{public}")),
            "{text}"
        );
        assert!(text.contains("interval_secs: 5"), "{text}");

        c.ui.nav("/settings?tab=connections").await;
        edit_mode(c).await;
        c.ui.wait_for("[data-schema='pool/min_idle'] [data-editor]", SECS)
            .await;
        open_more(c, "pool/min_idle").await;
        let _: bool = c
            .ui
            .eval("(() => { const b = document.querySelector('[data-sticky-bar]').getBoundingClientRect(); const r = document.querySelector(\"[data-schema='pool/min_idle']\").getBoundingClientRect(); scrollBy(0, r.bottom - b.top - 10); document.getElementById('f-pool-min-idle').focus(); return true; })()")
            .await;
        clear_of_bar(c, "pool/min_idle").await;
        c.ui.fill("#f-pool-min-idle", "100").await;
        let err = row_error(c, "pool/min_idle").await;
        assert!(err.contains("max_idle"), "{err}");
        clear_of_bar(c, "pool/min_idle").await;
        assert!(
            !c.ui.enabled("[data-testid=save]").await,
            "a server error in a setting blocks Add to changes"
        );
        c.ui.click("[data-testid=discard-edit]").await;
        c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
        c.ui.nav("/settings?tab=server").await;
        edit_mode(c).await;
        c.ui.wait_for("[data-server-address]", SECS).await;
        click_js(c, "[data-server-address] input[value=name]").await;
        c.ui.fill("#f-public-address", "bad host").await;
        let err = row_error(c, "public_address").await;
        assert!(!err.is_empty(), "a host with a space is refused inline");
        c.ui.shot("server-address-invalid").await;
        click_js(c, "[data-server-address] input[value=wss]").await;
        c.ui.fill("#f-public-address", "tunnel.example.net/burrow")
            .await;
        let err = row_error(c, "public_address").await;
        assert!(
            err.contains("no websocket section"),
            "a wss:// address is refused inline on a server without WebSocket, as validate refuses it: {err}"
        );
        c.ui.shot("server-address-wss").await;
        click_js(c, "[data-server-address] input[value=name]").await;
        c.ui.fill("#f-public-address", "tunnel.example.net").await;
        row_clean(c, "public_address").await;
        open_review(c, "review-of-the-server-address").await;
        applied(c, "server-address-applied").await;
        let text = c.world.config();
        assert!(
            text.contains("public_address: tunnel.example.net"),
            "{text}"
        );

        c.ui.nav("/settings?tab=connections").await;
        let peers = "[data-schema='udp/max_peers']";
        open_more(c, "udp/max_peers").await;
        let help =
            c.ui.wait_text(peers, "Maximum UDP senders per port", SECS)
                .await;
        assert!(
            help.contains("For example 20000; the default is 4096"),
            "{help}"
        );
        edit_mode(c).await;
        c.ui.wait_for(&format!("{peers} [data-editor]"), SECS).await;
        let _: bool = c
            .ui
            .eval("(() => { const b = document.querySelector('[data-sticky-bar]').getBoundingClientRect(); const r = document.querySelector(\"[data-schema='udp/max_peers']\").getBoundingClientRect(); scrollBy(0, r.bottom - b.top - 10); document.getElementById('f-udp-max-peers').focus(); return true; })()")
            .await;
        clear_of_bar(c, "udp/max_peers").await;
        c.ui.fill("#f-udp-max-peers", "0").await;
        let err = row_error(c, "udp/max_peers").await;
        assert!(!err.is_empty(), "0 peers is refused inline");
        c.ui.fill("#f-udp-max-peers", "1000001").await;
        let err = row_error(c, "udp/max_peers").await;
        assert!(!err.is_empty(), "more than 1000000 peers is refused inline");
        c.ui.fill("#f-udp-max-peers", "20000").await;
        row_clean(c, "udp/max_peers").await;
        c.ui.shot("udp-max-peers").await;
        open_review(c, "review-of-udp-max-peers").await;
        applied(c, "udp-max-peers-applied").await;
        let text = c.world.config();
        assert!(text.contains("max_peers: 20000"), "{text}");
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_address_health_check_and_picker_editors_check_input_and_write_yaml() {
    run("editors", Options::default(), false, editors);
}

fn fallback_edit(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/capped").await;
        edit_mode(c).await;
        let row = "[data-schema='services/*/bandwidth']";
        c.ui.wait_text(row, "Checked by the server", SECS).await;
        c.ui.fill("#f-services-capped-bandwidth", "abc").await;
        wait_checked(c).await;
        c.ui.wait_text(&format!("{row} [aria-live]"), "abc", SECS)
            .await;
        c.ui.shot("server-rejects-abc").await;
        c.ui.fill("#f-services-capped-bandwidth", "50mbit").await;
        wait_checked(c).await;
        open_review(c, "review-50mbit").await;
        applied(c, "applied-50mbit").await;
        assert!(c.world.config().contains("bandwidth: 50mbit"));
        c.ui.assert_no_errors();
    })
}

#[test]
fn e8_a_field_of_a_newer_type_is_edited_as_text_and_checked_by_the_server() {
    let opts = Options {
        server_env: vec![("BURROW_ADMIN_TEST_SCHEMA".into(), "future".into())],
        extra_services: "  capped:\n    client: home\n    target: 8080\n    visitors: [laptop]\n    bandwidth: 40mbit\n".into(),
        ..Options::default()
    };
    run("e8-fallback-edit", opts, true, fallback_edit);
}

async fn read_only_in_view(c: &mut Ctx, field: &str) {
    let row = format!("[data-schema='{field}']");
    open_more(c, field).await;
    assert_eq!(
        c.ui.count(&format!("{row} [data-editor]")).await,
        0,
        "{field} has no edit control"
    );
    assert_eq!(
        c.ui.count(&format!("{row} input, {row} select")).await,
        0,
        "{field} has no input"
    );
    assert_eq!(
        c.ui.count(&format!("{row} [data-locked]")).await,
        0,
        "{field} reads as a plain value in view mode, not a box"
    );
    let text =
        c.ui.wait_text(&row, "Change this in server.yaml (burrow edit)", SECS)
            .await;
    assert_eq!(
        text.contains("then restart"),
        field == "listen",
        "{field}: only listen needs a restart: {text}"
    );
    assert_eq!(
        text.matches("Change this").count(),
        1,
        "{field} says it once"
    );
}

fn locked_fields(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        for (tab, fields) in [
            ("server", &["listen", "insecure_plaintext"][..]),
            (
                "connections",
                &[
                    "heartbeat/interval_secs",
                    "limits/handshake_timeout_secs",
                    "limits/max_handshakes",
                ][..],
            ),
        ] {
            c.ui.nav(&format!("/settings?tab={tab}")).await;
            c.ui.wait_for("[data-testid=settings] [data-card]", SECS)
                .await;
            for field in fields {
                read_only_in_view(c, field).await;
            }
        }
        c.ui.nav("/settings").await;
        c.ui.wait_for("[data-schema=listen]", SECS).await;
        assert_eq!(
            c.ui.count("[data-schema=public_address] input").await,
            0,
            "view mode shows values"
        );
        edit_mode(c).await;
        assert!(
            c.ui.text("[data-schema=listen]")
                .await
                .contains("Change this in server.yaml"),
            "a locked address keeps its help while editing: {}",
            c.ui.text("[data-schema=listen]").await
        );
        assert!(
            c.ui.count("[data-schema=public_address] input").await > 0,
            "unlocked fields are editable in edit mode"
        );
        let public = c.ui.text("[data-schema=public_address]").await;
        assert!(
            public.contains("when WebSocket is on; connect commands made here use it"),
            "the public address keeps its whole help while editing: {public}"
        );
        c.ui.click("[data-testid=settings-tabs] [data-tab=connections]")
            .await;
        c.ui.wait_for("[data-card=websocket]", SECS).await;
        assert!(
            c.ui.count("[data-card=websocket] #f-websocket").await > 0,
            "WebSocket is turned on here (owner 0y)"
        );
        open_more(c, "limits/max_pending").await;
        assert!(
            c.ui.count("[data-schema='limits/max_pending'] input").await > 0,
            "the rest of limits stays editable"
        );
        c.ui.shot("locked-fields-are-read-only").await;
        assert_eq!(c.ui.count("[data-testid=review-open]").await, 0);
        c.ui.assert_no_errors();
    })
}

#[test]
fn locked_server_fields_are_read_only() {
    run("locked-fields", Options::default(), false, locked_fields);
}

async fn create_in_picker(c: &mut Ctx, field: &str, name: &str, by_button: bool) {
    c.ui.click(&format!("#{field}")).await;
    c.ui.wait_for("[role=listbox]", SECS).await;
    c.ui.click("[data-testid=combo-new]").await;
    c.ui.wait_for("[data-testid=combo-new-name]", SECS).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        c.ui.count("[role=listbox]").await,
        1,
        "{field}: the list stays open after New"
    );
    let focused: String =
        c.ui.eval("(document.activeElement && document.activeElement.dataset.testid) || ''")
            .await;
    assert_eq!(
        focused, "combo-new-name",
        "{field}: the name input has the focus"
    );
    let placeholder: String =
        c.ui.eval("document.querySelector('[data-testid=combo-new-name]').placeholder")
            .await;
    assert!(
        !placeholder.is_empty(),
        "{field}: the name input says what it is for"
    );
    c.ui.shot(&format!("{field}-new-name")).await;
    let el =
        c.ui.page
            .find_element("[data-testid=combo-new-name]")
            .await
            .expect("name input");
    el.type_str(name).await.expect("type the name");
    if by_button {
        c.ui.click("[data-testid=combo-create]").await;
    } else {
        el.press_key("Enter").await.expect("Enter");
    }
    c.ui.wait_for(&format!("[data-chip='{name}']"), SECS).await;
    assert_eq!(
        c.ui.count("[data-testid=combo-new-name]").await,
        0,
        "{field}: the name input goes once the item is made"
    );
    c.ui.shot(&format!("{field}-created")).await;
}

fn picker_create(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web?tab=settings").await;
        edit_mode(c).await;
        click_js(c, "#f-services-web-reach-visitors").await;
        c.ui.wait_for("[data-way=visitors] #f-services-web-visitors", SECS)
            .await;
        create_in_picker(c, "f-services-web-visitors", "tablet", false).await;
        create_in_picker(c, "f-services-web-clients", "edge", true).await;
        c.ui.click("#f-services-web-visitors").await;
        c.ui.wait_for("[role=listbox]", SECS).await;
        c.ui.click("[data-testid=combo-new]").await;
        c.ui.wait_for("[data-testid=combo-new-name]", SECS).await;
        let _: bool = c
            .ui
            .eval("(() => { document.querySelector('[data-testid=combo-new-name]').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); return true; })()")
            .await;
        c.ui.wait_gone("[data-testid=combo-new-name]", SECS).await;
        assert_eq!(
            c.ui.count("[role=listbox]").await,
            1,
            "Escape leaves New first"
        );
        c.ui.press("#f-services-web-visitors", "Escape").await;
        c.ui.wait_gone("[role=listbox]", SECS).await;
        c.ui.click("#f-services-web-visitors").await;
        c.ui.wait_for("[role=listbox]", SECS).await;
        assert_eq!(
            c.ui.count("[data-testid=combo-new-name]").await,
            0,
            "a reopened list starts without the name input"
        );
        c.ui.click("main h1").await;
        c.ui.wait_gone("[role=listbox]", SECS).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_new_visitor_or_client_is_made_from_its_picker_without_closing_it() {
    run("picker-create", Options::default(), false, picker_create);
}
