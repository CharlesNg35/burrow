#![cfg(feature = "web")]

use std::path::Path;

use crate::config::load::load_str_in;
use crate::config::settings::ServerSettings;
use crate::config::{Format, Loaded, Outcome, Role};

pub(super) const SECRET: &str = "yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=";

fn tls_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec!["tunnel.example.com".to_string()]).unwrap();
    let cert = params.self_signed(&key).unwrap();
    std::fs::write(dir.path().join("cert.pem"), cert.pem()).unwrap();
    std::fs::write(dir.path().join("key.pem"), key.serialize_pem()).unwrap();
    dir
}

fn load_in(dir: &Path, rest: &str) -> Outcome {
    let text = format!("clients:\n  home: {{ secret: \"{SECRET}\" }}\n{rest}");
    load_str_in(
        dir,
        "server.yaml",
        &text,
        Format::Yaml,
        Some(Role::Server),
        &|_: &str| None,
    )
}

pub(super) fn load(rest: &str) -> Outcome {
    load_in(Path::new("."), rest)
}

pub(super) fn errors(o: &Outcome) -> Vec<String> {
    o.report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.clone())
        .collect()
}

pub(super) fn settings(o: Outcome) -> ServerSettings {
    let errs = errors(&o);
    match o.loaded {
        Some(Loaded::Server(s, _)) => *s,
        _ => panic!("expected a valid server config: {errs:#?}"),
    }
}

fn one_error(rest: &str) -> String {
    let o = load(rest);
    let e = errors(&o);
    assert_eq!(e.len(), 1, "{e:#?}");
    e[0].clone()
}

pub(super) const NAS: &str = "services:\n  nas:\n    client: home\n    target: 192.168.1.20:443\n    web: { hosts: [NAS.example.com], tls: passthrough }\n";

#[test]
fn a_passthrough_site_turns_into_settings_on_a_dual_stack_port() {
    let s = settings(load(&format!("listen: 7835\nweb:\n  https: 443\n{NAS}")));
    let web = s.web.unwrap();
    assert_eq!(web.https, Some("[::]:443".parse().unwrap()));
    assert_eq!(web.site_of("nas").unwrap().hosts, ["nas.example.com"]);
    assert_eq!(web.tunnel, None);
    let v4 = settings(load(&format!(
        "listen: 7835\nweb: {{ https: 0.0.0.0:443 }}\n{NAS}"
    )));
    assert_eq!(v4.web.unwrap().https, Some("0.0.0.0:443".parse().unwrap()));
}

#[test]
fn a_site_without_a_top_level_web_is_refused() {
    let e = one_error(&format!("listen: 7835\n{NAS}"));
    assert_eq!(
        e,
        "services.nas.web: the web proxy is off; add a top-level web: with its port, like web: {https: 443}"
    );
}

#[test]
fn a_site_that_needs_https_from_burrow_is_refused_until_burrow_can_serve_it() {
    for web in [
        "web: blog.example.com",
        "web: [a.example.com, b.example.com]",
        "web: { hosts: [blog.example.com] }",
    ] {
        let e = one_error(&format!(
            "listen: 7835\nweb: {{ https: 443 }}\nservices:\n  blog:\n    client: home\n    target: 8080\n    {web}\n"
        ));
        assert_eq!(
            e,
            "services.blog.web: burrow cannot serve HTTPS for websites in this build yet; use tls: passthrough",
            "{web}: the default with web.https is auto"
        );
    }
}

#[test]
fn every_final_tls_form_parses_and_only_passthrough_is_served_in_this_build() {
    let site = |tls: &str| {
        format!(
            "listen: 7835\nweb: {{ https: 443 }}\nservices:\n  blog:\n    client: home\n    target: 8080\n    web: {{ hosts: [blog.example.com], tls: {tls} }}\n"
        )
    };
    let https_later = "services.blog.web.tls: burrow cannot serve HTTPS for websites in this build yet; use tls: passthrough";
    assert_eq!(one_error(&site("auto")), https_later);
    assert_eq!(
        one_error(&site("{ cert: certs/blog.pem, key: certs/blog.key }")),
        https_later
    );
    assert_eq!(
        one_error(&site("off")),
        "services.blog.web.tls: plain HTTP websites need web.http, which this build does not have yet; use tls: passthrough"
    );
    assert!(errors(&load(&site("passthrough"))).is_empty());
    let half = errors(&load(&site("{ cert: certs/blog.pem }")));
    assert!(half.iter().any(|e| e.contains("key")), "{half:?}");
    let wrong = errors(&load(&site("sometimes")));
    assert!(!wrong.is_empty(), "an unknown word is refused");
}

#[test]
fn web_needs_its_https_port_in_this_build() {
    let e = one_error(&format!("listen: 7835\nweb: {{}}\n{NAS}"));
    assert_eq!(
        e,
        "web needs a port: https for HTTPS sites, like https: 443 (plain HTTP sites on web.http are not in this build yet)"
    );
}

#[test]
fn host_names_follow_the_name_rules() {
    let site = |hosts: &str| {
        format!(
            "listen: 7835\nweb: {{ https: 443 }}\nservices:\n  apps:\n    client: home\n    target: 443\n    web: {{ hosts: {hosts}, tls: passthrough }}\n"
        )
    };
    assert_eq!(
        one_error(&site("[\"https://blog.example.com\"]")),
        "services.apps.web.hosts: \"https://blog.example.com\" is not a host name; write the name alone, like blog.example.com"
    );
    assert_eq!(
        one_error(&site("[\"*.example.*\"]")),
        "services.apps.web.hosts: \"*.example.*\" is not a valid wildcard; a wildcard is \"*.\" followed by a name, like *.example.com"
    );
    assert_eq!(
        one_error(&site("[\"*.com\"]")),
        "services.apps.web.hosts: \"*.com\" is too wide; a wildcard needs a parent name, like *.example.com"
    );
    assert_eq!(
        one_error(&site("[]")),
        "services.apps.web.hosts: a website needs at least one host name"
    );
    assert!(one_error(&site("[a.example.com, A.example.com.]")).contains("twice"));
    let ok = settings(load(&site("[\"*.apps.example.com\", 203.0.113.10]")));
    assert_eq!(
        ok.web.unwrap().site_of("apps").unwrap().hosts,
        ["*.apps.example.com", "203.0.113.10"]
    );
}

#[test]
fn one_host_leads_to_one_service() {
    let e = one_error(&format!(
        "listen: 7835\nweb: {{ https: 443 }}\n{NAS}  nas2:\n    client: home\n    target: 443\n    web: {{ hosts: [nas.example.com], tls: passthrough }}\n"
    ));
    assert_eq!(
        e,
        "host \"nas.example.com\" is used by services \"nas\" and \"nas2\"; each host name leads to one service"
    );
}

#[test]
fn web_is_for_tcp_services_and_its_port_is_taken_from_others() {
    let udp = one_error(
        "listen: 7835\nweb: { https: 443 }\nservices:\n  dns:\n    client: home\n    target: 53\n    protocol: udp\n    web: { hosts: [dns.example.com], tls: passthrough }\n",
    );
    assert_eq!(
        udp,
        "service \"dns\" is udp, but web only works for tcp services; remove web:, or set protocol: tcp"
    );
    let bind = one_error(&format!(
        "listen: 7835\nweb: {{ https: 8443 }}\n{NAS}  other:\n    client: home\n    bind: 8443\n    target: 80\n"
    ));
    assert!(bind.contains("the web proxy's HTTPS port"), "{bind}");
    let other_host = load(&format!(
        "listen: 7835\nweb: {{ https: 127.0.0.1:8443 }}\n{NAS}  other:\n    client: home\n    bind: 192.0.2.1:8443\n    target: 80\n"
    ));
    assert!(errors(&other_host).is_empty(), "{:?}", errors(&other_host));
    let udp_same_port = load(&format!(
        "listen: 7835\nweb: {{ https: 8443 }}\n{NAS}  quic:\n    client: home\n    bind: 8443\n    target: 443\n    protocol: udp\n"
    ));
    assert!(
        errors(&udp_same_port).is_empty(),
        "{:?}",
        errors(&udp_same_port)
    );
    let control = one_error(&format!("listen: 443\nweb: {{ https: 443 }}\n{NAS}"));
    assert!(control.contains("is also the control port"), "{control}");
    let metrics = one_error(&format!(
        "listen: 7835\nmetrics: {{ listen: 127.0.0.1:443 }}\nweb: {{ https: 443 }}\n{NAS}"
    ));
    assert!(metrics.contains("web.https"), "{metrics}");
}

fn shared(listen: &str, https: &str, host: &str) -> Outcome {
    let dir = tls_dir();
    load_in(
        dir.path(),
        &format!(
            "websocket:\n  listen: {listen}\n{host}  tls: {{ cert: cert.pem, key: key.pem }}\nweb: {{ https: {https} }}\n{NAS}"
        ),
    )
}

#[test]
fn the_tunnel_shares_the_port_when_both_say_the_same_address() {
    let host = "  host: Tunnel.example.com\n";
    let bare = settings(shared("443", "443", host));
    let ws = bare.websocket.as_ref().unwrap();
    assert_eq!(ws.listen, "[::]:443".parse().unwrap());
    assert_eq!(ws.host.as_deref(), Some("tunnel.example.com"));
    assert_eq!(
        bare.web.unwrap().tunnel.as_deref(),
        Some("tunnel.example.com")
    );
    let v4 = settings(shared("0.0.0.0:443", "0.0.0.0:443", host));
    assert_eq!(v4.websocket.unwrap().listen, "0.0.0.0:443".parse().unwrap());
    for (listen, https) in [
        ("443", "0.0.0.0:443"),
        ("0.0.0.0:443", "443"),
        ("127.0.0.1:443", "443"),
    ] {
        let e = errors(&shared(listen, https, host));
        assert_eq!(e.len(), 1, "{listen} {https}: {e:#?}");
        assert!(
            e[0].contains("overlap; to share the port give both the same address"),
            "{e:?}"
        );
    }
    let apart = settings(load(&format!(
        "websocket: {{ listen: 127.0.0.1:7836 }}\nweb: {{ https: 443 }}\n{NAS}"
    )));
    assert_eq!(apart.websocket.unwrap().host, None);
}

#[test]
fn the_tunnel_messages_of_section_5_3() {
    assert_eq!(
        errors(&shared("443", "443", "")),
        [
            "websocket.host is required when websocket.listen is the same port as web.https (443): it is the name that leads to the tunnel"
        ]
    );
    let alone = load(&format!(
        "websocket: {{ listen: 7836, host: tunnel.example.com }}\nweb: {{ https: 443 }}\n{NAS}"
    ));
    assert_eq!(
        errors(&alone),
        [
            "websocket.host is only used when websocket.listen is the same port as web.https; remove it, or set websocket.listen to the port of web.https"
        ]
    );
    assert_eq!(
        errors(&shared("443", "443", "  host: nas.example.com\n")),
        [
            "websocket.host \"nas.example.com\" is a passthrough site; burrow cannot see its requests"
        ]
    );
    assert!(errors(&shared("443", "443", "  host: \"*.example.com\"\n"))[0].contains("wildcard"));
    let no_cert = load(&format!(
        "websocket: {{ listen: 443, host: tunnel.example.com }}\nweb: {{ https: 443 }}\n{NAS}"
    ));
    assert_eq!(
        errors(&no_cert),
        [
            "websocket.host \"tunnel.example.com\" has no certificate; add websocket.tls with its cert and key"
        ]
    );
}

#[test]
fn an_empty_web_key_needs_its_port_too() {
    let e = one_error(&format!("listen: 7835\nweb:\n{NAS}"));
    assert_eq!(
        e,
        "web needs a port: https for HTTPS sites, like https: 443 (plain HTTP sites on web.http are not in this build yet)"
    );
}

#[test]
fn the_tunnel_and_the_web_port_on_two_addresses_of_one_port_do_not_overlap() {
    let apart = settings(load(&format!(
        "websocket: {{ listen: 10.8.0.1:443 }}\nweb: {{ https: 203.0.113.10:443 }}\n{NAS}"
    )));
    let ws = apart.websocket.unwrap();
    assert_eq!(ws.listen, "10.8.0.1:443".parse().unwrap());
    assert_eq!(ws.host, None);
    assert_eq!(
        apart.web.unwrap().https,
        Some("203.0.113.10:443".parse().unwrap())
    );
    let overlap = errors(&load(&format!(
        "websocket: {{ listen: 0.0.0.0:443 }}\nweb: {{ https: 203.0.113.10:443 }}\n{NAS}"
    )));
    assert_eq!(overlap.len(), 1, "{overlap:?}");
    assert!(overlap[0].contains("overlap"), "{overlap:?}");
}
