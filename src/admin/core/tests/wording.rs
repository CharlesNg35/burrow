#![cfg(test)]

use burrow_admin_logic::wording::shown;
use burrow_admin_proto::codes::{
    CHALLENGE_PORT, DNS_UNCOVERED, DUPLICATE_BIND, NO_CERTIFICATE, NO_CHECK, PLAIN_HTTP,
    WILDCARD_NEEDS_DNS,
};
use burrow_admin_proto::diag::Diag;
use burrow_config::ServiceEntry;

use super::{core_for, world};
use crate::admin::core::plan;

fn diags(extra_top: &str, services: &str) -> Vec<Diag> {
    let w = world();
    let text = format!("{}{services}{extra_top}", w.text);
    let core = core_for(&text);
    plan::check_text(&core, &text).diags
}

fn one(d: &[Diag], code: &str) -> String {
    let d = d
        .iter()
        .find(|d| d.code == code)
        .unwrap_or_else(|| panic!("no {code} in {d:?}"));
    shown(&d.code, &d.message, &d.subjects)
}

#[test]
fn the_ui_names_the_subject_validate_reports() {
    let site =
        |name: &str, web: &str| format!("  {name}: {{ client: home, target: 3000, web: {web} }}\n");
    let d = diags(
        "web:\n  https: 8443\n  http: 8080\n",
        &(site("blog", "blog.example.com")
            + &site("wiki", "{ hosts: [wiki.example.com], tls: \"off\" }")),
    );
    assert!(one(&d, NO_CERTIFICATE).starts_with("blog.example.com has no certificate"));
    assert!(one(&d, PLAIN_HTTP).starts_with("wiki is served as plain HTTP"));
    let d = diags(
        "web:\n  https: 8443\n  letsencrypt: { email: ops@example.com }\n",
        &(site("blog", "blog.example.com") + &site("apps", "\"*.apps.example.com\"")),
    );
    assert!(one(&d, NO_CHECK).starts_with("Let's Encrypt cannot check blog.example.com,"));
    assert!(one(&d, WILDCARD_NEEDS_DNS).contains("*.apps.example.com only through a DNS check"));
    let d = diags("", "  web2: { client: home, bind: 7840, target: 3001 }\n");
    let clash = d
        .iter()
        .find(|d| d.code == DUPLICATE_BIND)
        .expect("a clash");
    assert!(
        clash.also.contains(&ServiceEntry::BIND.of("web")),
        "{clash:?}"
    );
}

#[test]
fn a_check_the_site_cannot_use_is_told_at_its_choice_in_plain_words() {
    let d = diags(
        "web:\n  https: 8443\n  http: 8080\n  letsencrypt:\n    email: ops@example.com\n    dns:\n      - domains: [example.com]\n        cloudflare: { token: \"${CF}\" }\n",
        "  blog: { client: home, target: 3000, web: { hosts: [blog.example.com], challenge: http } }\n  apps: { client: home, target: 3001, web: \"*.apps.example.org\" }\n",
    );
    let port = d
        .iter()
        .find(|x| x.code == CHALLENGE_PORT)
        .expect("the HTTP check has no port 80");
    assert_eq!(
        port.path.as_deref(),
        Some(
            &[
                "services".to_string(),
                "blog".into(),
                "web".into(),
                "challenge".into()
            ][..]
        )
    );
    assert!(
        one(&d, CHALLENGE_PORT).starts_with("The HTTP check needs the web proxy's HTTP port on 80")
    );
    let wild = one(&d, DNS_UNCOVERED);
    assert!(
        wild.contains("no DNS check provider lists apps.example.org"),
        "{wild}"
    );
    assert!(!wild.contains("web.letsencrypt"), "{wild}");
    let d = diags(
        "web:\n  https: 8443\n  http: 8080\n  letsencrypt: { email: ops@example.com }\nwebsocket:\n  listen: 8443\n  host: tunnel.example.com\n  challenge: http\n",
        "",
    );
    let tunnel = d
        .iter()
        .find(|x| x.code == CHALLENGE_PORT)
        .expect("the tunnel's HTTP check has no port 80");
    assert_eq!(
        tunnel.path.as_deref(),
        Some(&["websocket".to_string(), "challenge".into()][..])
    );
}
