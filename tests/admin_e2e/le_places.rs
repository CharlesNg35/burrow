use std::time::Duration;

use super::certificates::pair;
use super::edits::{Flow, click_js, row_error};
use super::letsencrypt::{FAKE_CA, apply_all, env, three, validates};
use super::world::Options;
use super::{Ctx, SECS, edit_mode, open, open_more, run, wait_checked};

const PROBLEM: &str = "[data-testid=new-website-tls-problem]";
const WEB: &str = "web:\n  https: 127.0.0.1:HTTPS_PORT\n  http: 127.0.0.1:HTTP_PORT\n";

fn le_on() -> String {
    format!(
        "{WEB}  letsencrypt:\n    email: ops@example.com\n    directory: {FAKE_CA}\n    dns:\n      - domains: [example.com]\n        cloudflare: {{ token: \"${{CF_TOKEN}}\" }}\n"
    )
}

async fn new_website(c: &Ctx) {
    c.ui.nav("/services").await;
    c.ui.click("[data-testid=new]").await;
    c.ui.wait_for("[data-new=website]", SECS).await;
    c.ui.click("[data-new=website]").await;
    c.ui.wait_for("[data-testid=new-website]", SECS).await;
}

async fn names(c: &Ctx, list: &[&str]) {
    let _: bool = c
        .ui
        .eval("(() => { document.querySelectorAll('[data-testid=new-website] [data-chip] button').forEach(b => { if (!b.closest('[data-testid=new-website-client]')) b.click(); }); return true; })()")
        .await;
    for n in list {
        c.ui.fill("#new-website-names", n).await;
        c.ui.press("#new-website-names", "Enter").await;
        c.ui.wait_for(&format!("[data-chip='{n}']"), SECS).await;
    }
}

async fn tls(c: &Ctx, v: &str) {
    click_js(
        c,
        &format!("[data-testid=new-website-tls] input[value={v}]"),
    )
    .await;
}

async fn problem(c: &Ctx, wanted: &str) -> String {
    let said = c.ui.wait_text(PROBLEM, wanted, SECS).await;
    assert_eq!(
        c.ui.count("[data-testid=new-website-problem]").await,
        0,
        "the HTTPS problem is told at its step, not again above the bar"
    );
    assert!(!c.ui.enabled("[data-testid=create]").await);
    said
}

async fn wizard_steps(c: &mut Ctx) {
    new_website(c).await;
    names(c, &["blog.example.com"]).await;
    let said = problem(c, "Let's Encrypt cannot check blog.example.com").await;
    assert!(said.contains("DNS check providers"), "{said}");
    let order: bool = c
        .ui
        .eval("(() => { const e = document.querySelector('[data-testid=new-website-email]'); const n = document.querySelector('[data-testid=new-website-name]'); return !!e && !!(e.compareDocumentPosition(n) & Node.DOCUMENT_POSITION_FOLLOWING); })()")
        .await;
    assert!(
        order,
        "the email step follows the HTTPS step, before the service name"
    );
    c.ui.fill("[data-testid=new-website-email] input", "ops@example.com")
        .await;
    three(c, "wizard-no-check").await;
    tls(c, "internal").await;
    let lan = problem(c, "is not a name on your network").await;
    assert!(lan.contains("name it like blog.lan"), "{lan}");
    three(c, "wizard-local-ca-public-name").await;
    names(c, &["nas.lan"]).await;
    c.ui.wait_gone(PROBLEM, SECS).await;
    tls(c, "files").await;
    names(c, &["files.example.com"]).await;
    c.ui.wait_gone(PROBLEM, SECS).await;
    c.ui.wait_true(
        "!document.querySelector('[data-testid=create]').disabled",
        "My own certificate can be picked in the wizard",
        SECS,
    )
    .await;
    three(c, "wizard-own-certificate").await;
    c.ui.click("[data-testid=create]").await;
    let input = "[data-schema='services/*/web/tls'] [data-testid=upload-input]";
    c.ui.wait_for(input, SECS).await;
    c.ui.wait_true(
        "(document.activeElement && document.activeElement.closest('[data-schema]') || {}).dataset?.schema === 'services/*/web/tls'",
        "the new site opens on its HTTPS setting",
        SECS,
    )
    .await;
    let errors =
        c.ui.all_attrs("[data-schema='services/*/web/tls'] [aria-live] p", "class")
            .await;
    assert!(
        errors.len() <= 1,
        "one message for the missing upload: {errors:?}"
    );
    three(c, "wizard-own-certificate-upload").await;
    c.ui.upload_file(input, "files.pem", &pair(&["files.example.com"]))
        .await;
    c.ui.wait_text(
        "[data-schema='services/*/web/tls'] [data-testid=upload-set]",
        "files.example.com",
        SECS,
    )
    .await;
    apply_all(c, "wizard-own-certificate").await;
    let text = c.world.config();
    assert!(
        text.contains("files.example.com") && text.contains("upload: pair-"),
        "{text}"
    );
    validates(c);
}

fn wizard(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        wizard_steps(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn the_new_website_wizard_tells_https_problems_at_its_step_and_takes_an_own_certificate() {
    let opts = Options {
        extra_top: WEB.into(),
        ..Options::default()
    };
    run("le-wizard", opts, false, wizard);
}

fn wizard_le_on(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        new_website(c).await;
        names(c, &["shop.example.org"]).await;
        let said = problem(c, "Let's Encrypt cannot check shop.example.org").await;
        assert!(said.contains("Settings, Web proxy"), "{said}");
        three(c, "wizard-on-no-check").await;
        names(c, &["blog.example.com"]).await;
        c.ui.wait_gone(PROBLEM, SECS).await;
        assert_eq!(
            c.ui.count("[data-testid=new-website-email]").await,
            0,
            "Let's Encrypt is on already"
        );
        three(c, "wizard-on-dns-covers").await;
        c.ui.click("[data-testid=create]").await;
        c.ui.wait_for("[data-testid=entity-list] [data-row=blog]", SECS)
            .await;
        apply_all(c, "wizard-on").await;
        validates(c);
        c.ui.assert_no_errors();
    })
}

#[test]
fn a_new_website_with_let_s_encrypt_on_is_checked_against_the_ports_and_dns_providers() {
    let opts = Options {
        extra_top: le_on(),
        server_env: env(),
        ..Options::default()
    };
    run("le-wizard-on", opts, false, wizard_le_on);
}

async fn wildcard_and_check(c: &mut Ctx) {
    c.ui.nav("/services/blog").await;
    c.ui.wait_for("[role=tablist]", SECS).await;
    c.ui.click("[role=tablist] [data-tab=settings]").await;
    c.ui.wait_for("[data-schema='services/*/web']", SECS).await;
    edit_mode(c).await;
    c.ui.fill("#f-services-blog-web-hosts", "*.apps.example.org")
        .await;
    c.ui.press("#f-services-blog-web-hosts", "Enter").await;
    wait_checked(c).await;
    let wild = row_error(c, "services/*/web").await;
    assert!(
        wild.contains("no DNS check provider lists apps.example.org")
            && wild.contains("DNS check providers")
            && !wild.contains("web.letsencrypt"),
        "{wild}"
    );
    three(c, "site-wildcard-uncovered").await;
    c.ui.click("[data-chip='*.apps.example.org'] button").await;
    open_more(c, "services/*/web/challenge").await;
    c.ui.choose("f-services-blog-web-challenge", "http").await;
    wait_checked(c).await;
    let check = row_error(c, "services/*/web/challenge").await;
    assert!(
        check.starts_with("The HTTP check needs the web proxy's HTTP port on 80"),
        "{check}"
    );
    let names: String = c
        .ui
        .eval("document.querySelector(\"[data-schema='services/*/web'] [aria-live]\")?.innerText ?? ''")
        .await;
    assert!(
        !names.contains("HTTP check"),
        "told once, at the check: {names}"
    );
    three(c, "site-http-check-no-port").await;
    click_js(c, "[data-schema='services/*/web/tls'] input[value=files]").await;
    let clear = "[data-schema='services/*/web/challenge'] [data-testid=clear-unused]";
    c.ui.wait_for(clear, SECS).await;
    assert_eq!(c.ui.text(clear).await.trim(), "Clear");
    wait_checked(c).await;
    let unused = row_error(c, "services/*/web/challenge").await;
    assert!(
        unused
            .starts_with("This site uses its own certificate, so Let's Encrypt does not check it"),
        "{unused}"
    );
    let msgs: Vec<String> = c
        .ui
        .eval("[...document.querySelectorAll(\"[data-schema='services/*/web/tls'] [aria-live] p\")].map(p => p.innerText)")
        .await;
    assert_eq!(
        msgs,
        ["Uploaded certificate: not set yet"],
        "one plain message for the missing upload"
    );
    three(c, "site-own-certificate").await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

async fn attention_links(c: &mut Ctx) {
    c.ui.nav("/").await;
    let blog = "[data-testid=attention] [data-item='cert:blog.example.com']";
    let text =
        c.ui.wait_text(blog, "cannot be issued", Duration::from_secs(30))
            .await;
    assert!(text.contains("Last try:"), "{text}");
    let old =
        c.ui.wait_text(
            "[data-testid=attention] [data-item='cert:old.example.com']",
            "expires in",
            SECS,
        )
        .await;
    assert!(
        old.contains("Replace the file with a renewed one") && !old.contains("Let's Encrypt"),
        "{old}"
    );
    three(c, "attention-certificates").await;
    c.ui.click(&format!("{blog} a")).await;
    c.ui.wait_true(
        "(document.activeElement && document.activeElement.closest('[data-schema]') || {}).dataset?.schema === 'services/*/web/tls'",
        "the item lands on the site's HTTPS setting",
        SECS,
    )
    .await;
    let here =
        c.ui.wait_text("[data-testid=website-cert-now]", "cannot be issued", SECS)
            .await;
    assert!(here.contains("Last try:"), "{here}");
    three(c, "attention-landed").await;
}

fn site(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        attention_links(c).await;
        wildcard_and_check(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn certificate_problems_read_plainly_at_their_setting_and_link_there() {
    let dir = tempfile::tempdir().expect("a folder");
    let (cert, key) = expiring(&["old.example.com"], 5);
    let (cp, kp) = (dir.path().join("old.pem"), dir.path().join("old.key"));
    std::fs::write(&cp, cert).expect("cert");
    std::fs::write(&kp, key).expect("key");
    let sites = format!(
        "  blog:\n    client: home\n    target: 3001\n    web: blog.example.com\n  old:\n    client: home\n    target: 3002\n    web: {{ hosts: [old.example.com], tls: {{ cert: {}, key: {} }} }}\n",
        cp.display(),
        kp.display()
    );
    let opts = Options {
        extra_top: le_on(),
        extra_services: sites,
        server_env: env(),
        ..Options::default()
    };
    run("le-site", opts, false, site);
    drop(dir);
}

fn civil(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u8::try_from(doy - (153 * mp + 2) / 5 + 1).expect("a day");
    let m = u8::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).expect("a month");
    let y = i32::try_from(yoe + era * 400 + i64::from(m <= 2)).expect("a year");
    (y, m, d)
}

pub(super) fn expiring(names: &[&str], days: i64) -> (String, String) {
    let today = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_secs()
        / 86_400;
    let (y, m, d) = civil(i64::try_from(today).expect("days") + days);
    let key = rcgen::KeyPair::generate().expect("a key");
    let mut p =
        rcgen::CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
            .expect("params");
    p.not_after = rcgen::date_time_ymd(y, m, d);
    let cert = p.self_signed(&key).expect("a certificate");
    (cert.pem(), key.serialize_pem())
}
