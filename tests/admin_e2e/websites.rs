use super::edits::{Flow, click_js, row_clean, row_error};
use super::world::Options;
use super::{Ctx, SECS, applied, edit_mode, open, open_review, run, wait_checked};

const WEB_TOP: &str = "web:\n  https: 127.0.0.1:HTTPS_PORT\n";
const NAS: &str = "  nas:\n    client: home\n    target: 192.168.1.20:443\n    web:\n      hosts: [nas.example.com, \"*.nas.example.com\"]\n      tls: passthrough\n";
const SITE: &str = "services/*/web";
const SITE_TLS: &str = "services/*/web/tls";

fn opts() -> Options {
    Options {
        extra_top: WEB_TOP.into(),
        extra_services: NAS.into(),
        ..Options::default()
    }
}

async fn both_widths(c: &mut Ctx, step: &str) {
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    c.ui.shot(step).await;
    c.ui.viewport(420, 900).await;
    c.ui.shot(&format!("{step}-phone")).await;
    c.ui.viewport(1440, 900).await;
}

async fn add_name(c: &Ctx, id: &str, name: &str) {
    c.ui.fill(&format!("#{id}"), name).await;
    c.ui.press(&format!("#{id}"), "Enter").await;
    c.ui.wait_for(&format!("[data-chip='{name}']"), SECS).await;
}

async fn read_only_pages(c: &mut Ctx) {
    c.ui.wait_text("[data-testid=websites]", "nas.example.com", SECS)
        .await;
    both_widths(c, "overview-websites").await;
    c.ui.nav("/settings").await;
    c.ui.wait_for("[data-schema=web] [data-testid=locked-hint]", SECS)
        .await;
    c.ui.wait_text("[data-schema='web/https']", "HTTPS port", SECS)
        .await;
    both_widths(c, "settings-web-proxy").await;
    edit_mode(c).await;
    assert_eq!(
        c.ui.count("[data-schema='web/https'] input").await,
        0,
        "the web proxy's port is changed in server.yaml only"
    );
    c.ui.shot("settings-web-proxy-edit").await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
    c.ui.nav("/services").await;
    c.ui.wait_text("[data-row=nas]", "nas.example.com", SECS)
        .await;
    both_widths(c, "services-list").await;
    c.ui.nav("/services/nas").await;
    c.ui.wait_text("[role=tablist]", "Settings", SECS).await;
    let path = c.ui.text("main").await;
    assert!(path.contains("2 names on the HTTPS port"), "{path}");
    both_widths(c, "service-nas-path").await;
    c.ui.click("[role=tablist] [data-tab=settings]").await;
    c.ui.wait_text(
        &format!("[data-schema='{SITE_TLS}']"),
        "The app does its own HTTPS",
        SECS,
    )
    .await;
    let visibility = c.ui.text("[data-schema='services/*/visibility']").await;
    assert!(visibility.contains("Website only"), "{visibility}");
    both_widths(c, "service-nas-settings").await;
}

fn websites(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        read_only_pages(c).await;
        edit_mode(c).await;
        c.ui.wait_for("[data-testid=website-editor]", SECS).await;
        assert_eq!(c.ui.count("[data-testid=pick-visitors]").await, 0);
        both_widths(c, "nas-edit").await;
        add_name(c, "f-services-nas-web-hosts", "files.example.com").await;
        c.ui.shot("nas-name-added").await;
        add_name(c, "f-services-nas-web-hosts", "https://bad.example.com").await;
        let err = row_error(c, SITE).await;
        assert!(err.contains("write the name alone"), "{err}");
        assert!(!err.starts_with("hosts"), "{err}");
        both_widths(c, "nas-bad-name").await;
        c.ui.click("[data-chip='https://bad.example.com'] button")
            .await;
        row_clean(c, SITE).await;
        click_js(c, "[data-schema='services/*/web/tls'] input[value=auto]").await;
        wait_checked(c).await;
        let err = row_error(c, SITE).await;
        assert!(err.contains("choose The app does its own HTTPS"), "{err}");
        assert!(!err.contains("services.nas.web"), "{err}");
        let blocked: bool = c
            .ui
            .eval("(() => { const b = document.querySelector('[data-testid=save]'); return !!b && (b.disabled || b.getAttribute('aria-disabled') === 'true'); })()")
            .await;
        assert!(
            blocked,
            "a problem found by the server blocks the save like a local one"
        );
        both_widths(c, "nas-https-not-yet").await;
        click_js(c, "[data-schema='services/*/web/tls'] input[value=off]").await;
        wait_checked(c).await;
        let err = row_error(c, SITE_TLS).await;
        assert!(
            err.contains("plain HTTP websites are not in this build yet"),
            "{err}"
        );
        both_widths(c, "nas-plain-http-not-yet").await;
        let files_off: bool = c
            .ui
            .eval("(() => { const i = document.querySelector(\"[data-schema='services/*/web/tls'] input[value=files]\"); return !!i && i.disabled; })()")
            .await;
        assert!(files_off, "own certificate files are set in server.yaml");
        click_js(
            c,
            "[data-schema='services/*/web/tls'] input[value=passthrough]",
        )
        .await;
        row_clean(c, SITE_TLS).await;
        open_review(c, "nas-review").await;
        applied(c, "nas-applied").await;
        let text = c.world.config();
        assert!(text.contains("files.example.com"), "{text}");
        assert!(text.contains("tls: passthrough"), "{text}");

        c.ui.nav("/services/web").await;
        c.ui.wait_for("[role=tablist]", SECS).await;
        c.ui.click("[role=tablist] [data-tab=settings]").await;
        edit_mode(c).await;
        c.ui.wait_for("[data-testid=website-editor]", SECS).await;
        both_widths(c, "web-not-a-website").await;
        add_name(c, "f-services-web-web-hosts", "app.example.com").await;
        click_js(
            c,
            "[data-schema='services/*/web/tls'] input[value=passthrough]",
        )
        .await;
        wait_checked(c).await;
        row_clean(c, SITE_TLS).await;
        both_widths(c, "web-made-a-website").await;
        open_review(c, "web-review").await;
        applied(c, "web-applied").await;
        let text = c.world.config();
        assert!(text.contains("app.example.com"), "{text}");
        assert!(text.contains("# keep this comment"), "{text}");
        c.ui.nav("/").await;
        c.ui.wait_text("[data-testid=websites]", "app.example.com", SECS)
            .await;
        both_widths(c, "overview-after").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn websites_are_shown_and_edited_in_the_admin_ui() {
    run("websites", opts(), false, websites);
}
