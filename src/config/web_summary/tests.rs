#![cfg(feature = "web")]

use super::*;
use crate::config::{Format, Loaded, Role, load::load_str_in};

fn settings(dir: &std::path::Path, rest: &str) -> ServerSettings {
    let text = format!(
        "clients:\n  home: {{ secret: \"yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=\" }}\n{rest}"
    );
    let o = load_str_in(
        dir,
        "server.yaml",
        &text,
        Format::Yaml,
        Some(Role::Server),
        &|_: &str| None,
    );
    match o.loaded {
        Some(Loaded::Server(s, _)) => *s,
        _ => panic!("valid: {:?}", o.report.diagnostics),
    }
}

#[test]
fn the_summary_lists_the_port_the_tunnel_and_each_name() {
    let dir = tempfile::tempdir().unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["tunnel.example.com".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    std::fs::write(dir.path().join("cert.pem"), cert.pem()).unwrap();
    std::fs::write(dir.path().join("key.pem"), key.serialize_pem()).unwrap();
    let s = settings(
        dir.path(),
        "websocket:\n  listen: 443\n  host: tunnel.example.com\n  tls: { cert: cert.pem, key: key.pem }\nweb: { https: 443 }\nservices:\n  nas:\n    client: home\n    target: 192.168.1.20:443\n    web: { hosts: [nas.example.com, \"*.nas.example.com\"], tls: passthrough }\n  git:\n    client: self\n    target: 3000\n    web: { hosts: [git.example.com], tls: passthrough }\n",
    );
    let sum = summary(&s).unwrap();
    assert_eq!(
        sum.lines(),
        [
            "web: https [::]:443 (IPv4 and IPv6; shared with the tunnel)",
            "tunnel: wss://tunnel.example.com/burrow (websocket.tls cert.pem)",
            "*.nas.example.com: passthrough, the app's own certificate -> service nas",
            "git.example.com: passthrough, the app's own certificate -> service git",
            "nas.example.com: passthrough, the app's own certificate -> service nas",
        ]
        .map(String::from)
        .map(|l| l.replace(
            "cert.pem",
            &dir.path().join("cert.pem").display().to_string()
        ))
    );
    let json = serde_json::to_value(&sum).unwrap();
    assert_eq!(json["https"], "[::]:443");
    assert_eq!(json["hosts"][1]["service"], "git");
    assert_eq!(json["tunnel"]["url"], "wss://tunnel.example.com/burrow");
    assert_eq!(
        s.public_address.as_deref(),
        Some("wss://tunnel.example.com/burrow"),
        "client-config gets the shared tunnel's URL"
    );
}

#[test]
fn an_ipv4_port_without_names_says_so() {
    let s = settings(
        std::path::Path::new("."),
        "listen: 7835\nweb: { https: 0.0.0.0:8443 }\nservices:\n  x: { client: home, bind: 9000, target: 1 }\n",
    );
    assert_eq!(
        summary(&s).unwrap().lines(),
        [
            "web: https 0.0.0.0:8443 (IPv4 only)",
            "no names yet: every connection is closed",
        ]
    );
    assert_eq!(s.public_address, None);
    let off = settings(
        std::path::Path::new("."),
        "listen: 7835\nservices:\n  x: { client: home, bind: 9000, target: 1 }\n",
    );
    assert!(summary(&off).is_none());
}

fn pair(dir: &std::path::Path, stem: &str, names: &[&str]) {
    let key = rcgen::KeyPair::generate().unwrap();
    let names = names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    let cert = rcgen::CertificateParams::new(names)
        .unwrap()
        .self_signed(&key)
        .unwrap();
    std::fs::write(dir.join(format!("{stem}.pem")), cert.pem()).unwrap();
    std::fs::write(dir.join(format!("{stem}.key")), key.serialize_pem()).unwrap();
}

#[test]
fn every_host_says_how_it_is_served_with_its_certificate_source_and_check() {
    let dir = tempfile::tempdir().unwrap();
    pair(dir.path(), "corp", &["*.corp.example.com"]);
    pair(dir.path(), "portal", &["portal.example.com"]);
    let s = settings(
        dir.path(),
        "listen: 7835\nweb:\n  https: 443\n  http: 80\n  letsencrypt: { email: ops@example.com, profile: shortlived }\n  certificates:\n    - { hosts: [\"*.corp.example.com\"], cert: corp.pem, key: corp.key }\nservices:\n  blog: { client: home, target: 8080, web: example.com }\n  erp: { client: home, target: 8081, web: erp.corp.example.com }\n  portal:\n    client: home\n    target: 8082\n    web: { hosts: [portal.example.com], tls: { cert: portal.pem, key: portal.key } }\n  kiosk: { client: home, target: 8083, web: 203.0.113.10 }\n  intranet:\n    client: home\n    target: 8084\n    web: { hosts: [dash.lan.example.com], tls: off }\n  updates:\n    client: home\n    target: 8085\n    web: { hosts: [updates.example.com], https_redirect: false }\n  nas:\n    client: home\n    target: 443\n    web: { hosts: [nas.example.com], tls: passthrough }\n",
    );
    let at = |f: &str| dir.path().join(f).display().to_string();
    assert_eq!(
        summary(&s).unwrap().lines(),
        [
            "web: https [::]:443 (IPv4 and IPv6), http [::]:80 (IPv4 and IPv6)".to_string(),
            "certificates: Let's Encrypt (production) as ops@example.com, profile shortlived"
                .to_string(),
            "203.0.113.10: Let's Encrypt short-lived (6 days), TLS check -> service kiosk"
                .to_string(),
            "dash.lan.example.com: plain HTTP on [::]:80 only, unencrypted (tls: off) -> service intranet"
                .to_string(),
            format!("erp.corp.example.com: web.certificates[0] ({}) -> service erp", at("corp.pem")),
            "example.com: Let's Encrypt, TLS check -> service blog".to_string(),
            "nas.example.com: passthrough, the app's own certificate -> service nas".to_string(),
            format!("portal.example.com: own file {} -> service portal", at("portal.pem")),
            "updates.example.com: Let's Encrypt, TLS check; http:// served too (https_redirect: false) -> service updates"
                .to_string(),
            "http [::]:80: HTTPS hosts redirect to https:// (308); Let's Encrypt HTTP checks answered"
                .to_string(),
        ]
    );
    let plain = summary(&s).unwrap();
    let dash = plain
        .hosts
        .iter()
        .find(|h| h.service == "intranet")
        .unwrap();
    assert!(dash.unencrypted());
    assert!(
        !plain
            .hosts
            .iter()
            .find(|h| h.service == "blog")
            .unwrap()
            .unencrypted()
    );
}

#[test]
fn without_https_every_site_is_plain_and_the_tunnel_names_its_certificate() {
    let s = settings(
        std::path::Path::new("."),
        "listen: 7835\nweb:\n  http: 8080\nservices:\n  wiki: { client: home, target: 3000, web: wiki.lan.example.com }\n",
    );
    assert_eq!(
        summary(&s).unwrap().lines(),
        [
            "web: http [::]:8080 (IPv4 and IPv6)",
            "wiki.lan.example.com: plain HTTP on [::]:8080 only, unencrypted -> service wiki",
        ]
    );
    let t = settings(
        std::path::Path::new("."),
        "web:\n  https: 443\n  letsencrypt: { email: ops@example.com, directory: staging }\nwebsocket:\n  listen: 443\n  host: tunnel.example.com\n",
    );
    assert_eq!(
        summary(&t).unwrap().lines()[..3],
        [
            "web: https [::]:443 (IPv4 and IPv6; shared with the tunnel)",
            "certificates: Let's Encrypt (staging) as ops@example.com",
            "tunnel: wss://tunnel.example.com/burrow (Let's Encrypt, TLS check)",
        ]
    );
}

#[test]
fn a_tunnel_on_the_plain_http_port_says_it_is_burrow_encrypted() {
    let s = settings(
        std::path::Path::new("."),
        "web:\n  http: 80\nwebsocket:\n  listen: 80\n  host: tunnel.example.com\n",
    );
    assert_eq!(
        summary(&s).unwrap().lines(),
        [
            "web: http [::]:80 (IPv4 and IPv6; shared with the tunnel)",
            "tunnel: ws://tunnel.example.com/burrow (plain ws://, burrow-encrypted)",
        ]
    );
    assert_eq!(
        s.public_address.as_deref(),
        Some("ws://tunnel.example.com/burrow")
    );
}
