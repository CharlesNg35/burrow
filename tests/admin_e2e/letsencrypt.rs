use std::time::Duration;

use super::edits::{Flow, click_js};
use super::world::Options;
use super::{Ctx, SECS, applied, edit_mode, open, open_more, open_review, run, wait_checked};

pub(super) const ENV: &[(&str, &str)] = &[
    ("CF_TOKEN", "cf-token-for-the-e2e"),
    (
        "TSIG_SECRET",
        "c2VjcmV0c2VjcmV0c2VjcmV0c2VjcmV0c2VjcmV0MTI=",
    ),
    ("ACMEDNS_USER", "acmedns-user-e2e"),
    ("ACMEDNS_KEY", "acmedns-key-e2e"),
];
pub(super) const FAKE_CA: &str = "https://127.0.0.1:9/directory";
const WEB: &str = "web:\n  https: 127.0.0.1:HTTPS_PORT\n  http: 127.0.0.1:HTTP_PORT\n";
const APPS: &str = "  apps:\n    client: home\n    target: 9000\n    visitors: [laptop]\n";
const DNS: &str = "#f-web-letsencrypt-dns";

pub(super) fn env() -> Vec<(String, String)> {
    ENV.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

pub(super) async fn three(c: &mut Ctx, step: &str) {
    tokio::time::sleep(Duration::from_millis(300)).await;
    for w in [1440, 1280, 390] {
        c.ui.viewport(w, 900).await;
        c.ui.shot(&format!("{step}-{w}")).await;
    }
    c.ui.viewport(1440, 900).await;
}

pub(super) fn web_block(text: &str) -> String {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line == "web:" {
            inside = true;
        } else if inside && !line.is_empty() && !line.starts_with(' ') {
            break;
        }
        if inside {
            out.push(line);
        }
    }
    out.join("\n").trim_end().to_string()
}

pub(super) fn ported(c: &Ctx, want: &str) -> String {
    want.replace("HTTPS_PORT", &c.world.ports["https"].to_string())
        .replace("HTTP_PORT", &c.world.ports["http"].to_string())
}

pub(super) fn saved_web_is(c: &Ctx, want: &str) {
    let text = c.world.config();
    let want = ported(c, want);
    assert_eq!(
        web_block(&text),
        want.trim_end(),
        "the saved server.yaml:\n{text}"
    );
    validates(c);
}

pub(super) fn validates(c: &Ctx) {
    let out = std::process::Command::new(&c.world.server_bin)
        .args(["validate", "-c"])
        .arg(c.world.file("server.yaml"))
        .current_dir(c.world.path())
        .env("XDG_STATE_HOME", c.world.file("state"))
        .envs(ENV.iter().copied())
        .output()
        .expect("burrow validate");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "burrow validate refused it:\n{said}");
    for (_, secret) in ENV {
        assert!(!said.contains(secret), "validate printed a secret:\n{said}");
    }
}

pub(super) async fn web_tab(c: &Ctx) {
    c.ui.nav("/settings?tab=proxy").await;
    c.ui.wait_for("[data-schema=web]", SECS).await;
}

pub(super) async fn chip(c: &Ctx, id: &str, name: &str) {
    c.ui.fill(&format!("#{id}"), name).await;
    c.ui.press(&format!("#{id}"), "Enter").await;
    c.ui.wait_for(
        &format!("#{id} [data-chip='{name}'], [data-chip='{name}']"),
        SECS,
    )
    .await;
}

pub(super) async fn variable(c: &Ctx, id: &str, name: &str) {
    click_js(c, &format!("[name='{id}-source'][value=variable]")).await;
    c.ui.fill(&format!("#{id}"), name).await;
}

pub(super) async fn apply_all(c: &mut Ctx, step: &str) {
    wait_checked(c).await;
    open_review(c, &format!("{step}-review")).await;
    let review: String = c.ui.eval("document.body.innerText").await;
    for (_, secret) in ENV {
        assert!(!review.contains(secret), "Review shows a secret: {review}");
    }
    applied(c, &format!("{step}-applied")).await;
}

async fn t1_lets_encrypt_on(c: &mut Ctx) {
    web_tab(c).await;
    assert!(
        !c.ui
            .eval::<bool>(
                "!!document.querySelector(\"[data-card='web.letsencrypt.dns']\")?.offsetParent"
            )
            .await,
        "the DNS check providers appear only with Let's Encrypt on"
    );
    three(c, "t1-before").await;
    edit_mode(c).await;
    click_js(c, "#f-web-letsencrypt").await;
    c.ui.wait_for("#f-web-letsencrypt-email", SECS).await;
    c.ui.wait_for(
        "[data-card='web.letsencrypt.dns'] #f-web-letsencrypt-dns-add-button",
        SECS,
    )
    .await;
    c.ui.fill("#f-web-letsencrypt-email", "ops@example.com")
        .await;
    open_more(c, "web/letsencrypt/directory").await;
    c.ui.select("#f-web-letsencrypt-directory", "\u{1}other")
        .await;
    c.ui.fill("#f-web-letsencrypt-directory-text", FAKE_CA)
        .await;
    wait_checked(c).await;
    three(c, "t1-filled").await;
    apply_all(c, "t1").await;
    saved_web_is(
        c,
        "web:\n  https: 127.0.0.1:HTTPS_PORT\n  http: 127.0.0.1:HTTP_PORT\n  letsencrypt:\n    email: ops@example.com\n    directory: https://127.0.0.1:9/directory\n",
    );
    web_tab(c).await;
    three(c, "t1-saved").await;
}

async fn t2_cloudflare(c: &mut Ctx) {
    web_tab(c).await;
    edit_mode(c).await;
    c.ui.click(&format!("{DNS}-add-button")).await;
    c.ui.wait_for(&format!("{DNS}-0-domains"), SECS).await;
    three(c, "t2-new-entry").await;
    chip(c, "f-web-letsencrypt-dns-0-domains", "example.com").await;
    c.ui.select(&format!("{DNS}-0-provider"), "0").await;
    c.ui.wait_for(&format!("{DNS}-0-cloudflare-token"), SECS)
        .await;
    let help =
        c.ui.text("[data-schema='web/letsencrypt/dns/*/cloudflare/token']")
            .await;
    assert!(help.contains("Zone:Read and DNS:Edit"), "{help}");
    assert_eq!(
        c.ui.count("[data-schema='web/letsencrypt/dns/*/rfc2136/server']")
            .await,
        0,
        "only the picked provider's fields show"
    );
    variable(c, "f-web-letsencrypt-dns-0-cloudflare-token", "CF_TOKEN").await;
    chip(c, "f-web-letsencrypt-dns-0-wildcards", "*.example.com").await;
    wait_checked(c).await;
    three(c, "t2-filled").await;
    apply_all(c, "t2").await;
    saved_web_is(
        c,
        "web:\n  https: 127.0.0.1:HTTPS_PORT\n  http: 127.0.0.1:HTTP_PORT\n  letsencrypt:\n    email: ops@example.com\n    directory: https://127.0.0.1:9/directory\n    dns:\n      - domains: [example.com]\n        cloudflare: { token: \"${CF_TOKEN}\" }\n        wildcards: [\"*.example.com\"]\n",
    );
    web_tab(c).await;
    c.ui.wait_text("[data-card='web.letsencrypt.dns']", "${CF_TOKEN}", SECS)
        .await;
    three(c, "t2-saved").await;
}

async fn t3_rfc2136(c: &mut Ctx) {
    web_tab(c).await;
    edit_mode(c).await;
    c.ui.click(&format!("{DNS}-add-button")).await;
    chip(c, "f-web-letsencrypt-dns-1-domains", "corp.example.org").await;
    c.ui.select(&format!("{DNS}-1-provider"), "1").await;
    c.ui.fill(&format!("{DNS}-1-rfc2136-server"), "192.168.1.2:53")
        .await;
    c.ui.fill(&format!("{DNS}-1-rfc2136-key-name"), "burrow-key")
        .await;
    variable(c, "f-web-letsencrypt-dns-1-rfc2136-secret", "TSIG_SECRET").await;
    wait_checked(c).await;
    three(c, "t3-filled").await;
    apply_all(c, "t3").await;
    let text = c.world.config();
    assert!(
        web_block(&text).ends_with("      - domains: [corp.example.org]\n        rfc2136: { server: 192.168.1.2:53, key_name: burrow-key, secret: \"${TSIG_SECRET}\" }"),
        "{text}"
    );
    validates(c);
    web_tab(c).await;
    three(c, "t3-saved").await;
}

async fn t7_wildcard_site(c: &mut Ctx) {
    c.ui.nav("/services/apps").await;
    c.ui.wait_for("[role=tablist]", SECS).await;
    c.ui.click("[role=tablist] [data-tab=settings]").await;
    edit_mode(c).await;
    click_js(c, "#f-services-apps-reach-web").await;
    chip(c, "f-services-apps-web-hosts", "*.example.com").await;
    open_more(c, "services/*/web/challenge").await;
    click_js(
        c,
        "[data-schema='services/*/web/challenge'] input[value=dns]",
    )
    .await;
    wait_checked(c).await;
    three(c, "t7-site").await;
    apply_all(c, "t7").await;
    let text = c.world.config();
    assert!(
        text.contains("  apps:\n    client: home\n    target: 9000\n    visitors: [laptop]\n    web:\n      hosts:\n        - \"*.example.com\"\n      challenge: dns\n"),
        "{text}"
    );
    validates(c);
    c.ui.nav("/").await;
    super::overview_tab(c, "certs").await;
    c.ui.wait_for("[data-testid=certs-card] [data-cert='*.example.com']", SECS)
        .await;
    three(c, "t7-certificates").await;
}

async fn t4_switch_to_acme_dns(c: &mut Ctx) {
    web_tab(c).await;
    edit_mode(c).await;
    c.ui.select(&format!("{DNS}-0-provider"), "2").await;
    c.ui.wait_for("[data-testid=one-of-switch]", SECS).await;
    let dialog = c.ui.text("[data-testid=one-of-switch]").await;
    assert!(dialog.contains("Cloudflare"), "{dialog}");
    three(c, "t4-confirm").await;
    c.ui.click("[data-testid=one-of-confirm]").await;
    c.ui.wait_for(&format!("{DNS}-0-acme-dns-api"), SECS).await;
    assert_eq!(
        c.ui.count(&format!("{DNS}-0-cloudflare-token")).await,
        0,
        "the Cloudflare token is gone with its provider"
    );
    c.ui.fill(&format!("{DNS}-0-acme-dns-api"), "https://auth.example.org")
        .await;
    variable(
        c,
        "f-web-letsencrypt-dns-0-acme-dns-username",
        "ACMEDNS_USER",
    )
    .await;
    variable(
        c,
        "f-web-letsencrypt-dns-0-acme-dns-password",
        "ACMEDNS_KEY",
    )
    .await;
    c.ui.fill(
        &format!("{DNS}-0-acme-dns-subdomain"),
        "d420c923-bbd7-4056-ab64-c3ca54c9b3cf",
    )
    .await;
    wait_checked(c).await;
    three(c, "t4-filled").await;
    apply_all(c, "t4").await;
    let web = web_block(&c.world.config());
    assert!(
        web.contains("      - domains: [example.com]\n        wildcards: [\"*.example.com\"]\n        acme_dns:\n          api: https://auth.example.org\n          username: \"${ACMEDNS_USER}\"\n          password: \"${ACMEDNS_KEY}\"\n          subdomain: d420c923-bbd7-4056-ab64-c3ca54c9b3cf\n"),
        "{web}"
    );
    assert!(!web.contains("cloudflare"), "{web}");
    validates(c);
    web_tab(c).await;
    three(c, "t4-saved").await;
}

async fn t5_remove_and_turn_off(c: &mut Ctx) {
    web_tab(c).await;
    edit_mode(c).await;
    c.ui.click("[data-card='web.letsencrypt.dns'] [data-item='1'] [data-testid=remove-item]")
        .await;
    c.ui.wait_gone("[data-card='web.letsencrypt.dns'] [data-item='1']", SECS)
        .await;
    wait_checked(c).await;
    three(c, "t5-removed").await;
    apply_all(c, "t5").await;
    assert!(!c.world.config().contains("corp.example.org"));
    validates(c);
    web_tab(c).await;
    edit_mode(c).await;
    click_js(c, "#f-web-letsencrypt").await;
    wait_checked(c).await;
    let warn =
        c.ui.wait_text(
            "[data-card='web.letsencrypt']",
            "would have no certificate",
            SECS,
        )
        .await;
    assert!(warn.contains("*.example.com"), "{warn}");
    assert!(
        !c.ui
            .eval::<bool>(
                "!!document.querySelector(\"[data-card='web.letsencrypt.dns']\")?.offsetParent"
            )
            .await,
        "the DNS check providers go with Let's Encrypt"
    );
    three(c, "t5-off-warning").await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

fn tasks(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        t1_lets_encrypt_on(c).await;
        t2_cloudflare(c).await;
        t3_rfc2136(c).await;
        t7_wildcard_site(c).await;
        t4_switch_to_acme_dns(c).await;
        t5_remove_and_turn_off(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn lets_encrypt_and_dns_checks_are_set_up_in_the_admin() {
    let opts = Options {
        extra_top: WEB.into(),
        extra_services: APPS.into(),
        server_env: env(),
        ..Options::default()
    };
    run("le-tasks", opts, false, tasks);
}
