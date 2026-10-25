#![cfg(test)]

use burrow_admin_logic::wording::shown;
use burrow_admin_proto::codes::{
    DUPLICATE_BIND, NO_CERTIFICATE, NO_CHECK, PLAIN_HTTP, WILDCARD_NEEDS_DNS,
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
    assert!(one(&d, NO_CHECK).starts_with("Let's Encrypt cannot check blog.example.com:"));
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
