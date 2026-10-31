use super::edits::{Flow, click_js, row_clean, row_error};
use super::world::Options;
use super::{
    Ctx, SECS, applied, both_widths, edit_mode, open, open_review, overview_tab, run, wait_checked,
};

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

async fn add_name(c: &Ctx, id: &str, name: &str) {
    c.ui.fill(&format!("#{id}"), name).await;
    c.ui.press(&format!("#{id}"), "Enter").await;
    c.ui.wait_for(&format!("[data-chip='{name}']"), SECS).await;
}

async fn passthrough_shows_what_it_leaves_out(c: &mut Ctx) {
    c.ui.nav("/services/nas?tab=settings").await;
    let group = "[data-testid=off-group]";
    let line =
        c.ui.wait_text(group, "the app does its own HTTPS here", SECS)
            .await;
    assert!(
        line.contains("settings need burrow to do the HTTPS"),
        "{line}"
    );
    both_widths(c, "passthrough-group-closed").await;
    c.ui.click(group).await;
    c.ui.wait_for("[data-off-group][open] [data-schema]", SECS)
        .await;
    both_widths(c, "passthrough-group-open").await;
    edit_mode(c).await;
    c.ui.wait_text(group, "the app does its own HTTPS here", SECS)
        .await;
    c.ui.shot("passthrough-group-edit").await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
    c.ui.nav("/settings").await;
    edit_mode(c).await;
    let listen = "[data-schema=listen]";
    let text = c.ui.wait_text(listen, "server.yaml", SECS).await;
    assert!(
        text.contains("burrow edit"),
        "a locked field says where: {text}"
    );
    assert_eq!(c.ui.count(&format!("{listen} [data-locked]")).await, 1);
    both_widths(c, "locked-field").await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

async fn read_only_pages(c: &mut Ctx) {
    c.ui.nav("/").await;
    overview_tab(c, "websites").await;
    c.ui.wait_text("[data-testid=websites]", "nas.example.com", SECS)
        .await;
    both_widths(c, "overview-websites").await;
    c.ui.click("[data-testid=overview-open]").await;
    c.ui.wait_for("[data-filter=websites][aria-pressed=true]", SECS)
        .await;
    c.ui.wait_for("[data-testid=entity-list] [data-row=nas]", SECS)
        .await;
    assert_eq!(
        c.ui.count("[data-testid=entity-list] [data-row=web]").await,
        0,
        "the Websites filter leaves out plain services"
    );
    both_widths(c, "services-websites-filter").await;
    c.ui.nav("/settings?tab=proxy").await;
    c.ui.wait_text("[data-schema='web/https']", "HTTPS port", SECS)
        .await;
    both_widths(c, "settings-web-proxy").await;
    edit_mode(c).await;
    assert!(
        c.ui.count("[data-schema='web/https'] input").await > 0,
        "the web proxy's ports are edited here (owner 0x)"
    );
    c.ui.shot("settings-web-proxy-edit").await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
    c.ui.nav("/services").await;
    c.ui.wait_text("[data-row=nas]", "nas.example.com", SECS)
        .await;
    let nas = c.ui.text("[data-row=nas]").await;
    assert!(
        nas.contains("website") && !nas.contains("private"),
        "a website row is tagged website, as Reached by rules: {nas}"
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
    let ways = c.ui.text("[data-testid=reached-by-view]").await;
    assert!(
        ways.contains("nas.example.com") && ways.contains("Off"),
        "one line per way in: {ways}"
    );
    both_widths(c, "service-nas-settings").await;
}

async fn website_more(c: &Ctx) -> bool {
    c.ui.eval("[...document.querySelectorAll('[data-more]')].some(d => !d.hidden && d.dataset.more.endsWith('.website'))")
        .await
}

const TOAST_OVER_BAR: &str = "(() => { const t = document.querySelector('[data-testid=toast]'); const bars = [...document.querySelectorAll('[data-sticky-bar]')]; if (!t || !bars.length) return -1; const a = t.getBoundingClientRect(); return bars.map(b => b.getBoundingClientRect()).filter(b => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom).length; })()";

async fn toast_clear_of_the_bar(c: &mut Ctx) {
    for (w, h) in [(1440, 900), (420, 900), (360, 740)] {
        c.ui.viewport(w, h).await;
        c.ui.wait_true(
            &format!("{TOAST_OVER_BAR} === 0"),
            "the toast settles clear of the save bar",
            SECS,
        )
        .await;
        let over: i64 = c.ui.eval(TOAST_OVER_BAR).await;
        assert_eq!(over, 0, "a toast covers the save bar at {w}px");
    }
    let scrolled: i64 = c
        .ui
        .eval(&format!("(async () => {{ scrollTo(0, 0); await new Promise(r => setTimeout(r, 400)); scrollTo(0, document.documentElement.scrollHeight); await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))); return {TOAST_OVER_BAR}; }})()"))
        .await;
    assert_eq!(
        scrolled, 0,
        "a toast covers the save bar right after a scroll to the end at 360px"
    );
    c.ui.shot("toast-clear-of-the-save-bar-phone").await;
    c.ui.viewport(1440, 900).await;
}

async fn plain_with_the_http_port(c: &mut Ctx) {
    let port = c.world.ports["http"];
    c.world
        .hand_edit(|t| t.replacen("web:\n", &format!("web:\n  http: 127.0.0.1:{port}\n"), 1));
    c.ui.nav("/settings?tab=proxy").await;
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
    let warn = row_error(c, SITE).await;
    assert!(warn.contains("served as plain HTTP"), "{warn}");
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

async fn own_certificate(c: &mut Ctx) {
    click_js(c, "[data-schema='services/*/web/tls'] input[value=files]").await;
    let input = "[data-schema='services/*/web/tls'] [data-testid=upload-input]";
    c.ui.wait_for(input, SECS).await;
    both_widths(c, "nas-own-certificate").await;
    let key = rcgen::KeyPair::generate().expect("a key");
    let names = ["nas.example.com", "*.nas.example.com", "files.example.com"];
    let cert = rcgen::CertificateParams::new(names.map(String::from).to_vec())
        .expect("params")
        .self_signed(&key)
        .expect("a certificate");
    c.ui.upload_file(input, "chain-only.pem", &cert.pem()).await;
    c.ui.wait_for(
        "[data-schema='services/*/web/tls'] [data-testid=upload-note]",
        SECS,
    )
    .await;
    both_widths(c, "nas-upload-refused").await;
    let pem = format!("{}{}", cert.pem(), key.serialize_pem());
    c.ui.upload_file(input, "files.pem", &pem).await;
    c.ui.wait_text(
        "[data-schema='services/*/web/tls'] [data-testid=upload-set]",
        "files.example.com",
        SECS,
    )
    .await;
    wait_checked(c).await;
    let shown: String = c.ui.eval("document.body.innerText").await;
    assert!(!shown.contains("PRIVATE KEY"), "the key is never shown");
    both_widths(c, "nas-uploaded").await;
}

fn websites(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        passthrough_shows_what_it_leaves_out(c).await;
        read_only_pages(c).await;
        edit_mode(c).await;
        c.ui.wait_for("[data-testid=website-editor]", SECS).await;
        c.ui.wait_for(
            "[data-testid=website-summary] [data-testid=edit-website]",
            SECS,
        )
        .await;
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
        own_certificate(c).await;
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
        click_js(c, "#f-services-web-reach-web").await;
        c.ui.wait_for("[data-testid=website-editor]", SECS).await;
        assert_eq!(
            c.ui.count("[data-schema='services/*/web/tls'] input").await,
            0,
            "the HTTPS choice appears only once the site has a name, so it never takes clicks that do nothing"
        );
        assert!(
            !website_more(c).await,
            "the site's More settings appear only once it has a name"
        );
        both_widths(c, "web-not-a-website").await;
        add_name(c, "f-services-web-web-hosts", "app.example.com").await;
        assert!(
            website_more(c).await,
            "a named site shows its More settings"
        );
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
        overview_tab(c, "websites").await;
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
        overview_tab(c, "certs").await;
        let row = "[data-testid=certs-card] [data-cert='portal.example.com']";
        let text =
            c.ui.wait_text(row, "File ", std::time::Duration::from_secs(20))
                .await;
        assert!(
            text.contains("Valid") && text.contains("portal.pem"),
            "{text}"
        );
        assert!(
            text.contains(" 4096") && !text.contains("years"),
            "a far expiry reads as a date: {text}"
        );
        both_widths(c, "overview-certificates").await;
        let pill =
            c.ui.text("[data-testid=overview-tabs] [data-tab=certs]")
                .await;
        assert!(
            pill.contains("Certificates") && pill.contains('1'),
            "{pill}"
        );
        assert_eq!(
            c.ui.count("[data-testid=overview] > div > [data-testid=certs-card]")
                .await,
            0
        );
        c.ui.click("[data-testid=overview-open]").await;
        c.ui.wait_for("[data-testid=settings]", SECS).await;
        c.ui.wait_for(
            "[data-testid=settings-tabs] [data-tab=proxy][aria-selected=true]",
            SECS,
        )
        .await;
        c.ui.wait_for("[data-testid=settings] [data-card=web]", SECS)
            .await;
        both_widths(c, "overview-open-certificates").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn certificates_are_listed_on_the_overview() {
    run("certificates", opts(), false, certificates);
}

fn local_ca(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.world.hand_edit(|t| {
            format!(
                "{t}  printer:\n    client: home\n    target: 8443\n    web:\n      hosts: [printer.lan]\n      tls: internal\n"
            )
        });
        c.ui.nav("/settings?tab=proxy").await;
        let print =
            c.ui.wait_text(
                "[data-testid=local-ca-fingerprint]",
                ":",
                std::time::Duration::from_secs(20),
            )
            .await;
        assert_eq!(print.split(':').count(), 32, "{print}");
        let sites = c.ui.text("[data-testid=local-ca-sites]").await;
        assert!(sites.contains("printer"), "{sites}");
        c.ui.wait_for("[data-testid=local-ca-download]", SECS).await;
        c.ui.eval::<bool>("(() => { document.querySelector('[data-card=local_ca]').scrollIntoView(); return true; })()")
            .await;
        both_widths(c, "settings-local-ca").await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_local_ca_shows_in_settings_with_its_fingerprint() {
    run("local-ca", opts(), false, local_ca);
}

fn web_off(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        c.ui.nav("/services/web").await;
        c.ui.wait_for("[role=tablist]", SECS).await;
        c.ui.click("[role=tablist] [data-tab=settings]").await;
        edit_mode(c).await;
        click_js(c, "#f-services-web-reach-web").await;
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
        overview_tab(c, "certs").await;
        let orders =
            c.ui.wait_text("[data-testid=cert-orders]", "blog", SECS)
                .await;
        assert!(orders.contains("next try"), "{orders}");
        c.ui.shot("profile-needs-attention").await;
        c.ui.click(&format!("{item} a")).await;
        c.ui.wait_for("[data-testid=settings]", SECS).await;
        c.ui.wait_true(
            "(document.activeElement && document.activeElement.closest('[data-schema]') || {}).dataset?.schema === 'web/letsencrypt/profile'",
            "the link lands on the profile",
            SECS,
        )
        .await;
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
        let service: String =
            c.ui.eval("document.getElementById('new-website-service').value")
                .await;
        assert_eq!(
            service, "shop",
            "the service name is prefilled from the first name"
        );
        let status = c.ui.text("[data-testid=new-website-service]").await;
        assert!(status.contains("Adds service shop"), "{status}");
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
        let shown: String =
            c.ui.eval("document.querySelector('#new-website-https').value")
                .await;
        assert_eq!(shown, "443", "the web proxy starts on the usual ports");
        both_widths(c, "new-website-web-off").await;
        let (https, http) = (c.world.ports["https"], c.world.ports["http"]);
        c.ui.fill("#new-website-https", &format!("127.0.0.1:{https}"))
            .await;
        c.ui.fill("#new-website-http", &format!("127.0.0.1:{http}"))
            .await;
        add_name(c, "new-website-names", "shop.example.com").await;
        c.ui.fill("#new-website-target", "8081").await;
        click_js(c, "[data-testid=new-website-tls] input[value=passthrough]").await;
        both_widths(c, "new-website-web-ports").await;
        c.ui.click("[data-testid=create]").await;
        c.ui.wait_for("[data-testid=entity-list] [data-row=shop]", SECS)
            .await;
        open_review(c, "new-website-web-on-review").await;
        applied(c, "new-website-web-on-applied").await;
        let text = c.world.config();
        assert!(
            text.contains(&format!("https: 127.0.0.1:{https}"))
                && text.contains(&format!("http: 127.0.0.1:{http}"))
                && text.contains("shop.example.com"),
            "{text}"
        );
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_new_website_on_a_server_without_web_turns_the_web_proxy_on() {
    run(
        "new-website-off",
        Options::default(),
        false,
        new_website_web_off,
    );
}
