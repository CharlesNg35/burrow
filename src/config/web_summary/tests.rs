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
            "web: https [::]:443 (HTTP/2 and HTTP/1.1, IPv4 and IPv6; shared with the tunnel)",
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
            "web: https 0.0.0.0:8443 (HTTP/2 and HTTP/1.1, IPv4 only)",
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
            "web: https [::]:443 (HTTP/2 and HTTP/1.1, IPv4 and IPv6), http [::]:80 (IPv4 and IPv6)".to_string(),
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
            "web: https [::]:443 (HTTP/2 and HTTP/1.1, IPv4 and IPv6; shared with the tunnel)",
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

#[test]
fn validate_shows_the_acme_account_url_once_the_state_folder_holds_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = settings(
        dir.path(),
        "listen: 7835\nweb:\n  https: 443\n  letsencrypt: { email: ops@example.com }\nservices:\n  shop: { client: home, target: 8081, web: shop.example.com }\n",
    );
    let state = dir.path().join("state");
    s.web
        .as_mut()
        .unwrap()
        .letsencrypt
        .as_mut()
        .unwrap()
        .state_dir = Some(state.clone());
    assert_eq!(
        account_url(&s),
        "created on the first start; burrow status and validate then show its URL for a CAA accounturi record"
    );
    let at = burrow_web::tls::acme::storage::account_dir(&state, LETSENCRYPT_PRODUCTION);
    std::fs::create_dir_all(&at).unwrap();
    let url = "https://acme-v02.api.letsencrypt.org/acme/acct/42";
    let key = pkcs8();
    std::fs::write(
        at.join("account.json"),
        format!("{{\"id\": \"{url}\", \"key_pkcs8\": \"{key}\"}}"),
    )
    .unwrap();
    let shown = account_url(&s);
    assert_eq!(
        shown,
        format!(
            "{url}; a CAA record can let only this account get certificates: issue \"letsencrypt.org; accounturi={url}\""
        )
    );
    assert!(!shown.contains(&key), "no key material");
    let summary = WebSummary {
        account: shown,
        ..summary(&s).unwrap()
    };
    assert!(
        summary.lines()[2].starts_with(&format!("account: {url}")),
        "{:?}",
        summary.lines()
    );
    std::fs::remove_file(at.join("account.json")).unwrap();
    std::fs::create_dir(at.join("account.json")).unwrap();
    assert!(
        account_url(&s).starts_with(&format!(
            "not shown, this user cannot read {}",
            state.display()
        )),
        "{}",
        account_url(&s)
    );
}

fn pkcs8() -> String {
    use base64::Engine;
    let key = rcgen::KeyPair::generate().unwrap();
    base64::prelude::BASE64_URL_SAFE_NO_PAD.encode(key.serialize_der())
}

#[test]
fn the_summary_lists_each_hosts_routes_redirects_and_the_proxies_in_front() {
    let dir = tempfile::tempdir().unwrap();
    let s = settings(
        dir.path(),
        "listen: 7835\nweb:\n  https: 443\n  http: 80\n  letsencrypt: { email: ops@example.com }\n  trusted_proxies: [173.245.48.0/20]\n  proxy_protocol_from: [10.0.0.0/8]\n  redirect_names:\n    old.example.com: \"https://example.com\"\n    example.com/shop: { to: \"https://shop.example.net/\", permanent: true }\nservices:\n  blog:\n    client: home\n    target: 8080\n    web:\n      hosts: [example.com]\n      challenge: http\n      redirect_from: [www.example.com]\n      redirects: { /feed: \"https://feeds.example.net/blog\", /2019: { to: /archive/2019, permanent: true } }\n  api:\n    client: home\n    target: 9000\n    web: { hosts: [example.com], paths: [/api], app_path: /v1, challenge: http }\n  docs:\n    client: home\n    target: 9001\n    web: { hosts: [docs.example.com], paths: [/guide], challenge: http, maintenance: true }\n",
    );
    let lines = summary(&s).unwrap().lines();
    let want = [
        "web: https [::]:443 (HTTP/2 and HTTP/1.1, IPv4 and IPv6), http [::]:80 (IPv4 and IPv6)",
        "PROXY headers required from 10.0.0.0/8; X-Forwarded-For believed from 173.245.48.0/20",
        "certificates: Let's Encrypt (production) as ops@example.com",
        "docs.example.com: Let's Encrypt, HTTP check",
        "  /guide      -> service docs (maintenance page)",
        "  everything else -> 404",
        "example.com: Let's Encrypt, HTTP check",
        "  /2019       -> redirect /archive/2019 (permanent)",
        "  /feed       -> redirect https://feeds.example.net/blog (temporary)",
        "  /shop       -> redirect https://shop.example.net/ (permanent)",
        "  /api        -> service api (app sees /v1, X-Forwarded-Prefix /api)",
        "  /           -> service blog",
        "old.example.com: Let's Encrypt, TLS check",
        "  /           -> redirect https://example.com (temporary)",
        "www.example.com: Let's Encrypt, HTTP check",
        "  /           -> redirect https://example.com (permanent)",
        "http [::]:80: HTTPS hosts redirect to https:// (308); Let's Encrypt HTTP checks answered",
    ];
    assert_eq!(lines, want.map(String::from));
}

#[test]
fn the_summary_names_each_dns_provider_and_lists_the_shared_wildcards() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cf.token"), "token\n").unwrap();
    std::fs::write(dir.path().join("hook"), "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            dir.path().join("hook"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let s = settings(
        dir.path(),
        "listen: 7835\nweb:\n  https: 443\n  letsencrypt:\n    email: ops@example.com\n    dns:\n      - domains: [example.com]\n        cloudflare: { token: \"file:cf.token\" }\n        wildcards: [\"*.example.com\", \"*.lab.example.com\"]\n      - domains: [corp.example.org]\n        resolvers: [1.1.1.1]\n        command: hook\nservices:\n  wiki:\n    client: home\n    target: 3001\n    web: wiki.example.com\n  erp:\n    client: home\n    target: 3002\n    web: { hosts: [erp.corp.example.org], challenge: dns }\n",
    );
    let lines = summary(&s).unwrap().lines();
    assert_eq!(
        lines[1],
        "certificates: Let's Encrypt (production) as ops@example.com; DNS checks through Cloudflare for example.com, nameservers found via the system resolvers; DNS checks through the command for corp.example.org, nameservers found via 1.1.1.1:53"
    );
    assert_eq!(
        lines[2..5],
        [
            "wildcard *.example.com (+ example.com): Let's Encrypt, DNS check; used by wiki",
            "wildcard *.lab.example.com (+ lab.example.com): Let's Encrypt, DNS check; used by nothing yet",
            "erp.corp.example.org: Let's Encrypt, DNS check through the command (corp.example.org) (challenge: dns) -> service erp",
        ]
        .map(String::from)
    );
    assert_eq!(
        lines[5],
        "wiki.example.com: Let's Encrypt shared wildcard *.example.com -> service wiki"
    );
}

#[test]
fn the_summary_shows_references_as_written_never_their_values() {
    let dir = tempfile::tempdir().unwrap();
    let text = "clients:\n  home: { secret: \"yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=\" }\nlisten: 7835\nweb:\n  https: 443\n  letsencrypt:\n    email: \"${EMAIL}\"\n  redirect_names:\n    old.example.com: \"https://example.com/${WHERE}\"\nservices:\n  blog:\n    client: home\n    target: 8080\n    web:\n      hosts: [example.com]\n      redirects: { /go: \"https://example.net/${WHERE}\" }\n";
    let env = |n: &str| match n {
        "EMAIL" => Some("hidden@example.com".to_string()),
        "WHERE" => Some("s3cr3t".to_string()),
        _ => None,
    };
    let o = load_str_in(
        dir.path(),
        "server.yaml",
        text,
        Format::Yaml,
        Some(Role::Server),
        &env,
    );
    let Some(Loaded::Server(s, _)) = o.loaded else {
        panic!("{:?}", o.report.diagnostics)
    };
    let shown = summary(&s).unwrap().lines().join("\n");
    assert!(shown.contains("as ${EMAIL}"), "{shown}");
    assert!(
        shown.contains("redirect https://example.net/${WHERE} (temporary)"),
        "{shown}"
    );
    assert!(
        shown.contains("redirect https://example.com/${WHERE} (temporary)"),
        "{shown}"
    );
    assert!(
        !shown.contains("s3cr3t") && !shown.contains("hidden@"),
        "{shown}"
    );
    assert!(!format!("{:?}", s.web.as_ref().unwrap().written).contains("s3cr3t"));
}
