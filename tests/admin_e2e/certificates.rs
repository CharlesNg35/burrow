use super::edits::{Flow, click_js, row_error};
use super::letsencrypt::{ENV, apply_all, chip, env, three, validates, web_block, web_tab};
use super::world::Options;
use super::{Ctx, SECS, edit_mode, open, run, wait_checked};

const DOCS: &str = "web:\n  https: 127.0.0.1:HTTPS_PORT\n  http: 127.0.0.1:HTTP_PORT\n  letsencrypt:\n    email: ops@example.com\n    directory: https://127.0.0.1:9/directory\n    dns:\n      - domains: [example.com, example.net] # these domains and every name under them\n        cloudflare: { token: \"${CF_TOKEN}\" } # quoted: ${...} inside { } must be a quoted string in YAML\n        wildcards: [\"*.example.com\"] # one shared certificate: example.com and *.example.com\n      - domains: [corp.example.org]\n        rfc2136: { server: \"192.168.1.2:53\", key_name: burrow-key, secret: \"${TSIG_SECRET}\" }\n";
const SITES: &str = "  wiki:\n    client: home\n    target: 3001\n    web: wiki.example.com # covered by *.example.com: orders nothing of its own\n  erp:\n    client: office\n    target: 8443\n    web:\n      hosts: [erp.corp.example.org]\n      challenge: dns # only reachable inside the LAN\n";
const ENTRY: &str = "[data-card='web.letsencrypt.dns']";

async fn t8_existing_entries(c: &mut Ctx) {
    web_tab(c).await;
    let card = c.ui.wait_text(ENTRY, "corp.example.org", SECS).await;
    for want in [
        "example.com",
        "example.net",
        "Cloudflare",
        "${CF_TOKEN}",
        "*.example.com",
        "RFC 2136",
        "192.168.1.2:53",
        "burrow-key",
        "${TSIG_SECRET}",
    ] {
        assert!(card.contains(want), "{want} is shown: {card}");
    }
    for (_, secret) in ENV {
        assert!(!card.contains(secret), "a secret shows: {card}");
    }
    three(c, "t8-view").await;
    edit_mode(c).await;
    three(c, "t8-edit").await;
    let before = c.world.config();
    c.ui.fill("#f-web-letsencrypt-dns-1-rfc2136-key-name", "burrow-key-2")
        .await;
    wait_checked(c).await;
    apply_all(c, "t8").await;
    let after = c.world.config();
    let expected = before.replace("key_name: burrow-key,", "key_name: burrow-key-2,");
    assert_ne!(before, expected, "the docs example names the key");
    let changed: Vec<(&str, &str)> = expected
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert!(
        changed.is_empty() && expected.lines().count() == after.lines().count(),
        "only the key name changed: {changed:#?}"
    );
    validates(c);
}

async fn t9_invalid_input(c: &mut Ctx) {
    web_tab(c).await;
    edit_mode(c).await;
    c.ui.click("#f-web-letsencrypt-dns-add-button").await;
    chip(c, "f-web-letsencrypt-dns-2-domains", "example.com").await;
    c.ui.select("#f-web-letsencrypt-dns-2-provider", "0").await;
    c.ui.wait_for("#f-web-letsencrypt-dns-2-cloudflare-token", SECS)
        .await;
    wait_checked(c).await;
    let overlap = row_error(c, "web/letsencrypt/dns/*/domains").await;
    assert!(
        overlap.contains("already listed in DNS provider 1"),
        "{overlap}"
    );
    let token = row_error(c, "web/letsencrypt/dns/*/provider").await;
    assert_eq!(token, "Cloudflare needs its API token");
    three(c, "t9-overlap-and-token").await;
    c.ui.click("[data-item='2'] [data-testid=remove-item]")
        .await;
    chip(c, "f-web-letsencrypt-dns-0-wildcards", "www.example.com").await;
    wait_checked(c).await;
    let wild =
        c.ui.wait_text(
            "[data-item='0'] [data-schema='web/letsencrypt/dns/*/wildcards']",
            "*.",
            SECS,
        )
        .await;
    assert!(wild.contains("is not a wildcard"), "{wild}");
    three(c, "t9-wildcard").await;
    c.ui.click("[data-testid=discard-edit]").await;
    c.ui.wait_gone("[data-testid=save-bar]", SECS).await;
}

async fn t10_read_only(c: &mut Ctx) {
    c.ui.goto("about:blank").await;
    open(c, "viewer").await;
    c.ui.wait_for("[data-testid=read-only-tag]", SECS).await;
    web_tab(c).await;
    let card = c.ui.wait_text(ENTRY, "corp.example.org", SECS).await;
    assert!(
        card.contains("Cloudflare") && card.contains("${CF_TOKEN}"),
        "{card}"
    );
    assert!(
        !c.ui.enabled("[data-testid=edit]").await,
        "Edit is disabled for read access"
    );
    assert_eq!(
        c.ui.count("[data-editor]").await,
        0,
        "plain values, no controls"
    );
    for card in ["web.letsencrypt", "web.letsencrypt.dns", "web.certificates"] {
        let sel = format!("[data-card='{card}']");
        assert_eq!(
            c.ui.count(&format!("{sel} input:not([type=hidden]), {sel} select, {sel} button[data-testid=remove-item]")).await,
            0,
            "{card} has no control for a read-only admin"
        );
    }
    three(c, "t10-read-only").await;
}

fn existing(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        t8_existing_entries(c).await;
        t9_invalid_input(c).await;
        t10_read_only(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn existing_dns_entries_show_edit_and_refuse_in_place() {
    let opts = Options {
        extra_top: DOCS.into(),
        extra_services: SITES.into(),
        server_env: env(),
        clients: Some(vec!["home", "office"]),
        ..Options::default()
    };
    run("le-existing", opts, false, existing);
}

fn pair(names: &[&str]) -> String {
    let key = rcgen::KeyPair::generate().expect("a key");
    let cert =
        rcgen::CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
            .expect("params")
            .self_signed(&key)
            .expect("a certificate");
    format!("{}{}", cert.pem(), key.serialize_pem())
}

async fn t6_own_certificates(c: &mut Ctx) {
    web_tab(c).await;
    let files =
        c.ui.wait_text(
            "[data-card='web.certificates']",
            "Files on the server",
            SECS,
        )
        .await;
    assert!(files.contains("files.example.com"), "{files}");
    three(c, "t6-before").await;
    edit_mode(c).await;
    c.ui.click("#f-web-certificates-add-button").await;
    chip(c, "f-web-certificates-1-hosts", "shop.example.com").await;
    let input = "[data-card='web.certificates'] [data-item='1'] [data-testid=upload-input]";
    c.ui.wait_for(input, SECS).await;
    three(c, "t6-new-entry").await;
    c.ui.upload_file(input, "shop.pem", &pair(&["shop.example.com"]))
        .await;
    c.ui.wait_text(
        "[data-item='1'] [data-testid=upload-set]",
        "shop.example.com",
        SECS,
    )
    .await;
    wait_checked(c).await;
    three(c, "t6-uploaded").await;
    apply_all(c, "t6-shared").await;
    let web = web_block(&c.world.config());
    assert!(
        web.contains("hosts: [shop.example.com]") && web.contains("upload: pair-"),
        "{web}"
    );
    validates(c);
    site_settings(c, "shop").await;
    click_js(c, "[data-schema='services/*/web/tls'] input[value=auto]").await;
    wait_checked(c).await;
    three(c, "t6-site-shared").await;
    apply_all(c, "t6-shop").await;
    assert!(
        c.world
            .config()
            .contains("    target: 3002\n    web: shop.example.com\n"),
        "{}",
        c.world.config()
    );
    validates(c);
    site_settings(c, "app").await;
    click_js(c, "[data-schema='services/*/web/tls'] input[value=files]").await;
    let site = "[data-schema='services/*/web/tls'] [data-testid=upload-input]";
    c.ui.wait_for(site, SECS).await;
    c.ui.upload_file(site, "app.pem", &pair(&["app.example.com"]))
        .await;
    c.ui.wait_text(
        "[data-schema='services/*/web/tls'] [data-testid=upload-set]",
        "app.example.com",
        SECS,
    )
    .await;
    wait_checked(c).await;
    three(c, "t6-site-own").await;
    apply_all(c, "t6-app").await;
    let text = c.world.config();
    assert!(text.contains("tls: { upload: pair-"), "{text}");
    assert!(!text.contains("shop.example.com], tls"), "{text}");
    validates(c);
}

async fn site_settings(c: &Ctx, name: &str) {
    c.ui.nav(&format!("/services/{name}")).await;
    c.ui.wait_for("[role=tablist]", SECS).await;
    c.ui.click("[role=tablist] [data-tab=settings]").await;
    edit_mode(c).await;
}

fn own(c: &mut Ctx) -> Flow<'_> {
    Box::pin(async move {
        open(c, "ops").await;
        t6_own_certificates(c).await;
        c.ui.assert_no_errors();
    })
}

#[test]
fn own_certificates_are_uploaded_and_used_by_sites() {
    let dir = tempfile::tempdir().expect("a folder for the files");
    let both = pair(&["files.example.com"]);
    let (cert, key) = both.split_at(both.find("-----BEGIN PRIVATE KEY").expect("a key"));
    let cert_path = dir.path().join("files.pem");
    let key_path = dir.path().join("files.key");
    std::fs::write(&cert_path, cert).expect("cert");
    std::fs::write(&key_path, key).expect("key");
    let top = format!(
        "web:\n  https: 127.0.0.1:HTTPS_PORT\n  http: 127.0.0.1:HTTP_PORT\n  certificates:\n    - {{ hosts: [files.example.com], cert: {}, key: {} }}\n",
        cert_path.display(),
        key_path.display()
    );
    let sites = "  files:\n    client: home\n    target: 3001\n    web: files.example.com\n  shop:\n    client: home\n    target: 3002\n    web: { hosts: [shop.example.com], tls: off }\n  app:\n    client: home\n    target: 3003\n    web: { hosts: [app.example.com], tls: off }\n";
    let opts = Options {
        extra_top: top,
        extra_services: sites.into(),
        server_env: env(),
        ..Options::default()
    };
    run("le-own", opts, false, own);
    drop(dir);
}
