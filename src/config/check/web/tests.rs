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

pub(super) fn load_in(dir: &Path, rest: &str) -> Outcome {
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

pub(super) fn paths_after(text: &str, field: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(field) {
        let at = from + i;
        from = at + field.len();
        let before = text[..at].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let path: String = text[from..]
            .trim_start_matches(' ')
            .chars()
            .take_while(|c| !matches!(c, ',' | '}' | ' ' | '\n' | '"'))
            .collect();
        if !path.is_empty() {
            found.push(path);
        }
    }
    found
}

pub(super) fn files_for(text: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let names: Vec<String> = text
        .split(|c: char| c.is_whitespace() || "[]{},:\"'#".contains(c))
        .filter(|t| {
            t.contains('.') && !t.contains('/') && !t.ends_with(".pem") && !t.ends_with(".key")
        })
        .filter_map(|t| {
            burrow_syntax::web::host_name(t, burrow_syntax::web::SingleLabel::Allowed).ok()
        })
        .collect();
    let names = if names.is_empty() {
        vec!["example.com".to_string()]
    } else {
        names
    };
    for (cert, key) in paths_after(text, "cert:")
        .into_iter()
        .zip(paths_after(text, "key:"))
    {
        let pair = rcgen::KeyPair::generate().unwrap();
        let made = rcgen::CertificateParams::new(names.clone())
            .unwrap()
            .self_signed(&pair)
            .unwrap();
        for (file, body) in [(cert, made.pem()), (key, pair.serialize_pem())] {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
    }
    dir
}

pub(super) fn load(rest: &str) -> Outcome {
    let dir = files_for(rest);
    load_in(dir.path(), rest)
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
        "services.nas.web: the web proxy is off; add a top-level web: with its ports, like web: {https: 443, http: 80}"
    );
}

#[test]
fn a_site_served_over_https_by_burrow_needs_a_certificate_source_and_nothing_else() {
    for web in [
        "web: blog.example.com",
        "web: [a.example.com, b.example.com]",
        "web: { hosts: [blog.example.com] }",
    ] {
        let config = |le: &str| {
            format!(
                "listen: 7835\nweb: {{ https: 443{le} }}\nservices:\n  blog:\n    client: home\n    target: 8080\n    {web}\n"
            )
        };
        let e = errors(&load(&config("")));
        assert!(
            !e.is_empty() && e.iter().all(|m| m.contains("has no certificate")),
            "{web}: the default with web.https is auto: {e:#?}"
        );
        let with = errors(&load(&config(", letsencrypt: { email: ops@example.com }")));
        assert!(with.is_empty(), "{web}: {with:#?}");
    }
}

#[test]
fn every_final_tls_form_parses_and_automatic_certificates_need_a_source() {
    let site = |tls: &str| {
        format!(
            "listen: 7835\nweb: {{ https: 443, http: 80 }}\nservices:\n  blog:\n    client: home\n    target: 8080\n    web: {{ hosts: [blog.example.com], tls: {tls} }}\n"
        )
    };
    let auto = errors(&load(&site("auto")));
    assert_eq!(auto.len(), 1, "{auto:#?}");
    assert!(auto[0].contains("has no certificate"), "{auto:#?}");
    for served in [
        "passthrough",
        "off",
        "{ cert: certs/blog.pem, key: certs/blog.key }",
    ] {
        let found = errors(&load(&site(served)));
        assert!(
            found.iter().all(|e| !e.contains("in this build yet")),
            "{served}: {found:?}"
        );
    }
    let half = errors(&load(&site("{ cert: certs/blog.pem }")));
    assert!(half.iter().any(|e| e.contains("key")), "{half:?}");
    let wrong = errors(&load(&site("sometimes")));
    assert!(!wrong.is_empty(), "an unknown word is refused");
}

const NEEDS_A_PORT: &str = "web needs a port: https for HTTPS sites, like https: 443, or http for plain HTTP sites, like http: 80";

#[test]
fn web_needs_an_https_or_an_http_port() {
    assert_eq!(
        one_error(&format!("listen: 7835\nweb: {{}}\n{NAS}")),
        NEEDS_A_PORT
    );
}

pub(super) fn coded(o: &Outcome, code: &str) -> Vec<String> {
    o.report
        .diagnostics
        .iter()
        .filter(|d| d.code == code)
        .map(|d| format!("{:?}: {}", d.severity, d.message))
        .collect()
}

#[test]
fn a_server_without_web_http_is_told_that_http_addresses_get_no_answer() {
    let o = load(&format!("listen: 7835\nweb: {{ https: 443 }}\n{NAS}"));
    assert_eq!(
        coded(&o, "no-http"),
        ["Warning: web.http is not set: http:// addresses get no answer"]
    );
    let both = load(&format!(
        "listen: 7835\nweb: {{ https: 443, http: 80 }}\n{NAS}"
    ));
    assert!(coded(&both, "no-http").is_empty());
    let tunnel_only = load(
        "listen: 7835\nweb: { https: 443, letsencrypt: { email: you@example.com } }\nwebsocket: { listen: 443, host: tunnel.example.com }\n",
    );
    assert!(coded(&tunnel_only, "no-http").is_empty());
}

#[test]
fn a_server_with_only_web_http_serves_every_site_as_plain_http() {
    let s = settings(load(
        "listen: 7835\nweb: { http: 80 }\nservices:\n  wiki:\n    client: home\n    target: 3000\n    web: wiki.lan.example.com\n",
    ));
    let web = s.web.unwrap();
    assert_eq!(web.https, None);
    assert_eq!(web.http, Some("[::]:80".parse().unwrap()));
    let wiki = web.site_of("wiki").unwrap();
    assert_eq!(wiki.tls, crate::config::settings::WebTls::Off);
    assert!(!wiki.https_redirect);
}

#[test]
fn a_plain_http_site_is_warned_that_its_traffic_is_open() {
    let site = |ports: &str, web: &str| {
        load(&format!(
            "listen: 7835\nweb: {{ {ports} }}\nservices:\n  wiki:\n    client: home\n    target: 3000\n    web: {web}\n"
        ))
    };
    let warned = [
        "Warning: services.wiki: served as plain HTTP; anyone on the path can read and change its traffic",
    ];
    assert_eq!(
        coded(&site("http: 80", "wiki.lan.example.com"), "plain-http"),
        warned
    );
    let help = |ports: &str, web: &str| {
        site(ports, web)
            .report
            .diagnostics
            .into_iter()
            .find(|d| d.code == "plain-http")
            .and_then(|d| d.help)
            .unwrap()
    };
    assert_eq!(
        help("http: 80", "wiki.lan.example.com"),
        "for a site reached over the internet, use HTTPS: add https: 443 under web"
    );
    assert_eq!(
        help(
            "https: 443, http: 80",
            "{ hosts: [wiki.example.com], tls: off }"
        ),
        "for a site reached over the internet, remove tls: off so burrow serves it over HTTPS"
    );
    assert_eq!(
        coded(
            &site(
                "https: 443, http: 80",
                "{ hosts: [wiki.example.com], tls: off }"
            ),
            "plain-http"
        ),
        warned
    );
    assert!(
        coded(
            &site(
                "https: 443, http: 80",
                "{ hosts: [wiki.example.com], tls: { cert: w.pem, key: w.key } }"
            ),
            "plain-http"
        )
        .is_empty()
    );
}

#[test]
fn an_https_choice_without_web_https_is_refused_per_site() {
    let o = load(
        "listen: 7835\nweb:\n  http: 8080\nservices:\n  a: {client: home, target: 1, web: {hosts: [a.example.com], tls: auto}}\n  b: {client: home, target: 2, web: {hosts: [b.example.com], tls: passthrough}}\n  c: {client: home, target: 3, web: {hosts: [c.example.com], tls: {cert: a.pem, key: a.key}}}\n  d: {client: home, target: 4, web: {hosts: [nas]}}\n  e: {client: home, target: 5, web: {hosts: [e.example.com], challenge: http}}\n",
    );
    let tail =
        "needs web.https; without web.https every site is plain HTTP. Add https: 443, or remove";
    assert_eq!(
        errors(&o),
        [
            format!("services.a.web.tls: auto {tail} tls"),
            format!("services.b.web.tls: passthrough {tail} tls"),
            format!("services.c.web.tls: {{cert, key}} {tail} tls"),
            format!("services.e.web.challenge: http {tail} challenge"),
        ]
    );
}

#[test]
fn an_invalid_web_https_address_gives_only_the_address_error() {
    let e = errors(&load(
        "listen: 7835\nweb: { https: \"nope\", http: 80 }\nservices:\n  a: {client: home, target: 1, web: {hosts: [a.example.com], tls: auto}}\n",
    ));
    assert_eq!(e.len(), 1, "{e:#?}");
    assert!(
        e[0].starts_with("invalid web.https address \"nope\""),
        "{e:#?}"
    );
}

#[test]
fn web_http_must_not_share_a_port_with_another_listener() {
    for (yaml, want) in [
        (
            "web: { https: 8443, http: 8443 }",
            "web.http [::]:8443 is also web.https [::]:8443",
        ),
        (
            "web: { http: 7835 }",
            "web.http [::]:7835 is also the control port (listen 0.0.0.0:7835)",
        ),
        (
            "web: { https: 443, http: 8080 }\nwebsocket: { listen: 0.0.0.0:8080 }",
            "web.http [::]:8080 is also websocket.listen 0.0.0.0:8080",
        ),
    ] {
        let found = errors(&load(&format!("listen: 7835\n{yaml}\n{NAS}")));
        assert!(found.iter().any(|e| e == want), "{yaml}: {found:?}");
    }
    let bound = errors(&load(
        "listen: 7835\nweb: { http: 8080 }\nservices:\n  db:\n    client: home\n    target: 5432\n    bind: 8080\n",
    ));
    assert!(
        bound
            .iter()
            .any(|e| e.contains("the web proxy's HTTP port")),
        "{bound:?}"
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
    let help = |host: &str| {
        let text = format!(
            "listen: 7835\nweb: {{ https: 443 }}\nservices:\n  a: {{ client: home, target: 443, web: {{ hosts: [\"{host}\"], tls: passthrough }} }}\n  b: {{ client: home, target: 443, web: {{ hosts: [\"{host}\"], tls: passthrough }} }}\n"
        );
        load(&text)
            .report
            .diagnostics
            .iter()
            .find(|d| d.code == "duplicate-host")
            .and_then(|d| d.help.clone())
            .unwrap()
    };
    assert_eq!(
        help("nas.example.com"),
        "remove it from one of them, or give service \"b\" a name of its own, like b.example.com"
    );
    assert_eq!(
        help("example.com"),
        "remove it from one of them, or give service \"b\" a name of its own, like b.example.com"
    );
    assert_eq!(help("203.0.113.10"), "remove it from one of them");
    for (first, second) in [("passthrough", "auto"), ("auto", "passthrough")] {
        let text = format!(
            "listen: 7835\nweb: {{ https: 443, letsencrypt: {{ email: ops@example.com }} }}\nservices:\n  a: {{ client: home, target: 443, web: {{ hosts: [nas.example.com], tls: {first} }} }}\n  b: {{ client: home, target: 443, web: {{ hosts: [nas.example.com], tls: {second} }} }}\n"
        );
        let pass = if first == "passthrough" { "a" } else { "b" };
        assert_eq!(
            one_error(&text),
            format!(
                "host \"nas.example.com\" is used by a passthrough service \"{pass}\" and by a website"
            )
        );
    }
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
            "websocket.host is only used when websocket.listen is the same port as web.https or web.http; remove it, or set websocket.listen: 443 (or 80)"
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
            "websocket.host \"tunnel.example.com\" has no certificate; add web.letsencrypt, websocket.tls, or an entry in web.certificates whose hosts lists it"
        ]
    );
}

#[test]
fn an_empty_web_key_needs_its_port_too() {
    assert_eq!(
        one_error(&format!("listen: 7835\nweb:\n{NAS}")),
        NEEDS_A_PORT
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
