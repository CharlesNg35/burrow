#![cfg(test)]

use burrow_admin_proto::msg::{AdminLevel, ErrorCode};
use burrow_admin_proto::ops::Op;
use serde_json::{Value, json};

use crate::admin::core::plan;
use crate::admin::core::*;

use super::{World, core_for, p, session, world};

struct Dns {
    w: World,
    ca: String,
    token: String,
    _dir: tempfile::TempDir,
}

fn dns_world() -> Dns {
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("cf.token").display().to_string();
    std::fs::write(&token, "cf-token\n").unwrap();
    let ca = dir.path().join("corp-ca.pem").display().to_string();
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    std::fs::write(&ca, params.self_signed(&key).unwrap().pem()).unwrap();
    let mut w = world();
    w.text = w.text.replace(
        "\nservices:\n",
        &format!(
            "\nweb:\n  https: 8443\n  http: 8080\n  letsencrypt:\n    email: ops@example.com\n    dns:\n      - domains: [example.com]\n        cloudflare: {{ token: 'file:{token}' }}\n\nservices:\n  app:\n    client: home\n    target: 9000\n    web: {{ hosts: [app.example.com], tls: off }}\n"
        ),
    );
    Dns {
        w,
        ca,
        token,
        _dir: dir,
    }
}

fn refused_at(core: &Core, s: &Session, ops: &[Op]) -> String {
    let base = text::rev(&core.read().unwrap());
    match plan::compute(core, s, &base, ops) {
        Err(e) if e.code == ErrorCode::LockedField => e.message,
        Err(e) => panic!("refused for another reason: {}", e.message),
        Ok(c) => panic!("not refused: {}", c.new_text),
    }
}

fn first(d: &Dns) -> Value {
    json!({"domains": ["example.com"], "cloudflare": {"token": format!("file:{}", d.token)}})
}

fn entry(d: &Dns, v: Value) -> Op {
    Op::Set {
        path: p(&["web", "letsencrypt", "dns"]),
        value: json!([first(d), v]),
    }
}

#[test]
fn a_dns_providers_command_is_changed_in_the_file_only() {
    let d = dns_world();
    let core = core_for(&d.w.text);
    let s = session(&d.w, AdminLevel::Write);
    for op in [
        Op::Set {
            path: p(&["web", "letsencrypt", "dns"]),
            value: json!([{"domains": ["example.com"], "command": "/bin/sh"}]),
        },
        entry(
            &d,
            json!({"domains": ["example.net"], "command": "/usr/local/bin/burrow-dns"}),
        ),
    ] {
        let message = refused_at(&core, &s, std::slice::from_ref(&op));
        assert!(message.contains("command"), "{message}");
    }
}

#[cfg(feature = "web")]
#[test]
fn dns_credentials_and_where_they_go_are_set_from_the_admin() {
    let d = dns_world();
    let core = core_for(&d.w.text);
    let s = session(&d.w, AdminLevel::Write);
    let base = || text::rev(&core.read().unwrap());
    for (op, written) in [
        (
            entry(
                &d,
                json!({"domains": ["example.net"], "cloudflare": {"token": "${CF_TOKEN}"}}),
            ),
            "${CF_TOKEN}",
        ),
        (
            entry(
                &d,
                json!({"domains": ["example.net"], "cloudflare": {"token": "cf-literal-token"}}),
            ),
            "cf-literal-token",
        ),
        (
            entry(
                &d,
                json!({"domains": ["example.io"], "acme_dns": {"api": "https://auth.example.org", "username": "${ACMEDNS_USER}", "password": "file:acmedns.key", "subdomain": "x"}}),
            ),
            "https://auth.example.org",
        ),
        (
            entry(
                &d,
                json!({"domains": ["example.org"], "rfc2136": {"server": "203.0.113.9:53", "key_name": "k", "secret": "${TSIG_SECRET}"}}),
            ),
            "203.0.113.9:53",
        ),
    ] {
        let c = plan::compute(&core, &s, &base(), std::slice::from_ref(&op));
        let c = c.unwrap_or_else(|e| panic!("{op:?}: {}", e.message));
        assert!(c.new_text.contains(written), "{}", c.new_text);
        assert!(
            !c.plan.diff.unified.contains("cf-literal-token")
                && !serde_json::to_string(&c.plan.changes)
                    .unwrap()
                    .contains("cf-literal-token"),
            "a written secret never shows in the review: {:?}",
            c.plan.diff.unified
        );
    }
}

#[test]
fn a_target_ca_set_with_the_whole_target_tls_is_refused() {
    let d = dns_world();
    let core = core_for(&d.w.text);
    let s = session(&d.w, AdminLevel::Write);
    for op in [
        Op::Set {
            path: p(&["services", "app", "web", "target_tls"]),
            value: json!({"ca": d.ca}),
        },
        Op::Set {
            path: p(&["services", "app", "web"]),
            value: json!({"hosts": ["app.example.com"], "tls": "off", "target_tls": {"ca": d.ca}}),
        },
    ] {
        let message = refused_at(&core, &s, std::slice::from_ref(&op));
        assert!(message.contains("target_tls.ca"), "{message}");
    }
}

#[cfg(feature = "web")]
#[test]
fn a_dns_entrys_domains_wildcards_and_wait_stay_editable() {
    let d = dns_world();
    let core = core_for(&d.w.text);
    let s = session(&d.w, AdminLevel::Write);
    let base = || text::rev(&core.read().unwrap());
    let dns = |extra: Value| {
        let mut e = first(&d);
        e.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        Op::Set {
            path: p(&["web", "letsencrypt", "dns"]),
            value: json!([e]),
        }
    };
    for op in [
        dns(json!({"domains": ["example.com", "example.net"]})),
        dns(json!({"wildcards": ["*.example.com"]})),
        dns(json!({"wait_secs": 300})),
    ] {
        let c = plan::compute(&core, &s, &base(), std::slice::from_ref(&op));
        assert!(c.is_ok(), "{op:?}: {:?}", c.err().map(|e| e.message));
    }
}
