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
