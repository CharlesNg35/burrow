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
    let nas = c.ui.text("[data-row=nas]").await;
    assert!(
        nas.contains("passthrough"),
        "a website says how it is served: {nas}"
    );
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

async fn toast_clear_of_the_bar(c: &mut Ctx) {
    for (w, h) in [(1440, 900), (420, 900)] {
        c.ui.viewport(w, h).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let gap: f64 = c
            .ui
            .eval("(() => { const t = document.querySelector('[data-testid=toast]'); const b = document.querySelector('[data-sticky-bar]'); if (!t || !b) return -9999; return b.getBoundingClientRect().top - t.getBoundingClientRect().bottom; })()")
            .await;
        assert!(
            gap >= 0.0,
            "a toast covers the save bar at {w}px by {gap}px"
        );
    }
    c.ui.viewport(1440, 900).await;
}

async fn plain_with_the_http_port(c: &mut Ctx) {
    let port = c.world.ports["http"];
    c.world
        .hand_edit(|t| t.replacen("web:\n", &format!("web:\n  http: 127.0.0.1:{port}\n"), 1));
    c.ui.nav("/settings").await;
    c.ui.wait_text("[data-schema='web/http']", &port.to_string(), SECS)
        .await;
    c.ui.nav("/services/nas").await;
    c.ui.wait_for("[role=tablist]", SECS).await;
    c.ui.click("[role=tablist] [data-tab=settings]").await;
    edit_mode(c).await;
    click_js(c, "[data-schema='services/*/web/tls'] input[value=off]").await;
    wait_checked(c).await;
    row_clean(c, SITE_TLS).await;
    add_name(c, "f-services-nas-web-hosts", "nas").await;
    wait_checked(c).await;
    row_clean(c, SITE).await;
    both_widths(c, "nas-plain-http").await;
    open_review(c, "nas-plain-review").await;
    applied(c, "nas-plain-applied").await;
    let text = c.world.config();
    assert!(text.contains("tls: \"off\""), "{text}");
    assert!(
        text.contains(", nas]"),
        "a single-label name on a plain site: {text}"
    );
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
        assert!(
            err.contains("has no certificate: turn on Let's Encrypt"),
            "{err}"
        );
        assert!(!err.contains("web.letsencrypt"), "{err}");
        let blocked: bool = c
            .ui
            .eval("(() => { const b = document.querySelector('[data-testid=save]'); return !!b && (b.disabled || b.getAttribute('aria-disabled') === 'true'); })()")
            .await;
        assert!(
            blocked,
            "a problem found by the server blocks the save like a local one"
        );
        both_widths(c, "nas-https-needs-a-certificate").await;
        click_js(c, "[data-schema='services/*/web/tls'] input[value=off]").await;
        wait_checked(c).await;
        let err = row_error(c, SITE_TLS).await;
        assert!(
            err.contains("plain HTTP needs the web proxy's HTTP port"),
            "{err}"
        );
        both_widths(c, "nas-plain-http-needs-the-port").await;
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
        toast_clear_of_the_bar(c).await;
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
        plain_with_the_http_port(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn websites_are_shown_and_edited_in_the_admin_ui() {
    run("websites", opts(), false, websites);
}

fn certificates(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        let key = rcgen::KeyPair::generate().expect("a key");
        let cert = rcgen::CertificateParams::new(vec!["portal.example.com".to_string()])
            .expect("params")
            .self_signed(&key)
            .expect("a certificate");
        c.world.write("portal.pem", &cert.pem());
        c.world.write("portal.key", &key.serialize_pem());
        c.world.hand_edit(|t| {
            format!(
                "{t}  portal:\n    client: home\n    target: 8443\n    web:\n      hosts: [portal.example.com]\n      tls: {{ cert: portal.pem, key: portal.key }}\n"
            )
        });
        c.ui.nav("/").await;
        let row = "[data-testid=certs-card] [data-cert='portal.example.com']";
        let text =
            c.ui.wait_text(row, "Own file", std::time::Duration::from_secs(20))
                .await;
        assert!(
            text.contains("Valid") && text.contains("portal.pem"),
            "{text}"
        );
        assert!(
            text.contains(" 4096") && !text.contains("years"),
            "a far expiry reads as a date: {text}"
        );
        c.ui.wait_text(
            "[data-testid=overview-tabs]",
            "portal",
            std::time::Duration::from_millis(1500),
        )
        .await;
        both_widths(c, "overview-certificates").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn certificates_are_listed_on_the_overview() {
    run("certificates", opts(), false, certificates);
}

fn web_off(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        c.ui.wait_for("[role=tablist]", SECS).await;
        c.ui.click("[role=tablist] [data-tab=settings]").await;
        edit_mode(c).await;
        c.ui.wait_text("[data-testid=web-off]", "The web proxy is off", SECS)
            .await;
        add_name(c, "f-services-web-web-hosts", "app.example.com").await;
        wait_checked(c).await;
        let err = row_error(c, SITE).await;
        assert!(
            err.contains("the web proxy is off") && err.contains("Settings, Web proxy"),
            "{err}"
        );
        both_widths(c, "web-off").await;
        c.ui.click("[data-testid=web-off-link]").await;
        c.ui.wait_for("[data-testid=settings] [data-schema=web]", SECS)
            .await;
        let path: String = c.ui.eval("location.pathname").await;
        assert!(path.ends_with("/settings"), "{path}");
        c.ui.shot("web-off-settings").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_website_with_the_web_proxy_off_links_to_turning_it_on() {
    run("web-off", Options::default(), false, web_off);
}

fn profile_link(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        let item = "[data-testid=attention] [data-item='cert:blog.example.com']";
        let text = c.ui.wait_text(item, "cannot be renewed", SECS).await;
        assert!(text.contains("does not offer the profile"), "{text}");
        let orders =
            c.ui.wait_text("[data-testid=cert-orders]", "blog", SECS)
                .await;
        assert!(orders.contains("next try"), "{orders}");
        c.ui.shot("profile-needs-attention").await;
        c.ui.click(&format!("{item} a")).await;
        c.ui.wait_for("[data-testid=settings]", SECS).await;
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let focused: String = c
            .ui
            .eval("(() => { const a = document.activeElement; const r = a && a.closest('[data-schema]'); return r ? r.getAttribute('data-schema') : ''; })()")
            .await;
        assert_eq!(
            focused, "web/letsencrypt/profile",
            "the link lands on the profile"
        );
        c.ui.shot("profile-field").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_profile_the_service_does_not_offer_links_to_the_field() {
    let opts = Options {
        server_env: vec![("BURROW_ADMIN_TEST_CERT".into(), "blog.example.com".into())],
        extra_top: "web:\n  https: 127.0.0.1:HTTPS_PORT\n  letsencrypt: { email: ops@example.com, directory: \"https://127.0.0.1:9/directory\", profile: tlsserver }\n".into(),
        ..Options::default()
    };
    run("profile-link", opts, true, profile_link);
}

fn new_website(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services").await;
        c.ui.click("[data-testid=new]").await;
        c.ui.wait_for("[data-testid=new-menu] [data-new=website]", SECS)
            .await;
        c.ui.shot("new-menu").await;
        c.ui.click("[data-new=website]").await;
        c.ui.wait_for("[data-testid=new-website]", SECS).await;
        add_name(c, "new-website-names", "shop.example.com").await;
        c.ui.fill("#new-website-target", "8081").await;
        c.ui.wait_for("[data-testid=new-website-email]", SECS).await;
        c.ui.fill("#new-website-email", "ops@example.com").await;
        let problem = c.ui.text("[data-testid=new-website-problem]").await;
        assert!(
            problem.contains("Let's Encrypt cannot check shop.example.com"),
            "this test's HTTPS port is not 443: {problem}"
        );
        assert!(!c.ui.enabled("[data-testid=create]").await);
        both_widths(c, "new-website-no-check").await;
        click_js(c, "[data-testid=new-website-tls] input[value=passthrough]").await;
        c.ui.wait_gone("[data-testid=new-website-email]", SECS)
            .await;
        let service = c.ui.text("[data-testid=new-website-service]").await;
        assert!(service.contains("\"shop\""), "{service}");
        c.ui.wait_text("[data-testid=entity-list] [data-row=__new]", "shop", SECS)
            .await;
        let server = c.ui.text("[data-testid=new-website-path]").await;
        assert!(
            server.contains("HTTPS 127.0.0.1:"),
            "the web port, not the tunnel's: {server}"
        );
        both_widths(c, "new-website").await;
        c.ui.click("[data-testid=create]").await;
        c.ui.wait_for("[data-testid=entity-list] [data-row=shop]", SECS)
            .await;
        open_review(c, "new-website-review").await;
        applied(c, "new-website-applied").await;
        let text = c.world.config();
        assert!(
            text.contains("  shop:\n")
                && text.contains("target: 8081")
                && text.contains("shop.example.com"),
            "{text}"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_new_website_is_made_from_the_services_page() {
    run("new-website", opts(), false, new_website);
}

fn new_website_web_off(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services").await;
        c.ui.click("[data-testid=new]").await;
        c.ui.click("[data-new=website]").await;
        c.ui.wait_for("[data-testid=web-off-new]", SECS).await;
        assert!(
            c.ui.text("[data-testid=web-off-new]")
                .await
                .contains("https: 443")
        );
        add_name(c, "new-website-names", "shop.example.com").await;
        assert!(!c.ui.enabled("[data-testid=create]").await);
        assert_eq!(
            c.ui.count("[data-testid=new-website-problem]").await,
            0,
            "the callout says it once"
        );
        both_widths(c, "new-website-web-off").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_new_website_with_the_web_proxy_off_says_which_lines_to_add() {
    run(
        "new-website-off",
        Options::default(),
        false,
        new_website_web_off,
    );
}
