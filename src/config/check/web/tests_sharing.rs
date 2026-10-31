use super::tests::load;

fn found(rest: &str, code: &str) -> Vec<String> {
    load(rest)
        .report
        .diagnostics
        .iter()
        .filter(|d| d.code == code)
        .map(|d| d.message.clone())
        .collect()
}

fn errors(rest: &str) -> Vec<String> {
    super::tests::errors(&load(rest))
}

const HOST: &str = "  host: tunnel.example.com\n";

fn ws(listen: &str, extra: &str) -> String {
    format!("websocket:\n  listen: {listen}\n{extra}")
}

#[test]
fn a_tunnel_name_needs_a_shared_web_port() {
    let none = format!(
        "listen: 7835\nweb: {{ https: 443, http: 80 }}\n{}",
        ws("7836", HOST)
    );
    assert_eq!(
        found(&none, "tunnel-host"),
        [
            "websocket.host is only used when websocket.listen is the same port as web.https or web.http; remove it, or set websocket.listen: 443 (or 80)"
        ]
    );
    let no_web = format!("listen: 7835\n{}", ws("7836", HOST));
    assert_eq!(
        found(&no_web, "tunnel-host"),
        [
            "websocket.host is only used when the tunnel shares a web port, and this server has no web: block; remove websocket.host, or add web: {https: 443} (and web.letsencrypt for an automatic certificate) to share 443"
        ]
    );
}

#[test]
fn the_tunnel_shares_the_plain_http_port_without_a_certificate() {
    let plain = format!("web: {{ http: 80 }}\n{}", ws("80", HOST));
    assert!(errors(&plain).is_empty(), "{:#?}", errors(&plain));
    let nameless = format!("web: {{ http: 80 }}\n{}", ws("80", ""));
    assert_eq!(
        found(&nameless, "tunnel-host"),
        [
            "websocket.host is required when websocket.listen is the same port as web.http (80): it is the name that leads to the tunnel"
        ]
    );
}

#[test]
fn a_web_block_without_sites_is_valid() {
    let only = format!(
        "web:\n  https: 443\n  letsencrypt: {{ email: you@example.com }}\n{}  path: /burrow\n",
        ws("443", HOST)
    );
    assert!(errors(&only).is_empty(), "{:#?}", errors(&only));
    assert!(errors("listen: 7835\nweb: { https: 443 }\n").is_empty());
}

#[test]
fn lets_encrypt_on_a_port_it_does_not_check_is_refused_per_name() {
    let site = |ports: &str| {
        format!(
            "listen: 7835\nweb:\n  {ports}\n  letsencrypt: {{ email: you@example.com }}\nservices:\n  blog: {{ client: home, target: 8080, web: blog.example.com }}\n"
        )
    };
    let e = found(&site("https: 8443"), "no-check");
    assert_eq!(
        e,
        [
            "blog.example.com (service \"blog\") cannot be checked by Let's Encrypt: open http: 80, add challenge: dns, or list a file for it in web.certificates"
        ]
    );
    let help = load(&site("https: 8443"))
        .report
        .diagnostics
        .iter()
        .find(|d| d.code == "no-check")
        .and_then(|d| d.help.clone())
        .unwrap_or_default();
    assert!(help.contains("router"), "{help}");
    assert!(found(&site("https: 8443\n  http: 80"), "no-check").is_empty());
    assert!(found(&site("https: 443"), "no-check").is_empty());
    let tunnel = format!(
        "web:\n  https: 8443\n  letsencrypt: {{ email: you@example.com }}\n{}",
        ws("8443", HOST)
    );
    assert_eq!(
        found(&tunnel, "no-check"),
        [
            "websocket.host \"tunnel.example.com\" cannot be checked: open http: 80, set websocket.challenge: dns, or give websocket.tls files"
        ]
    );
}

#[test]
fn plain_websocket_on_443_gets_a_note() {
    let plain = format!(
        "listen: 7835\npublic_address: tunnel.example.com\n{}",
        ws("443", "")
    );
    let o = load(&plain);
    let note = o
        .report
        .diagnostics
        .iter()
        .find(|d| d.code == "plain-ws-443")
        .unwrap();
    assert!(!note.is_error());
    assert_eq!(
        note.message,
        "websocket.listen 443 has no TLS: clients must use ws://tunnel.example.com:443/burrow, and networks that expect TLS on 443 may block it; add websocket.tls, use web.letsencrypt (a web: block with websocket.host), or use port 80"
    );
    assert!(found(&format!("listen: 7835\n{}", ws("7836", "")), "plain-ws-443").is_empty());
}

#[test]
fn only_a_tunnel_on_the_https_port_needs_a_certificate_source() {
    let on80 = format!("web: {{ http: 80 }}\n{}", ws("80", HOST));
    assert!(found(&on80, "tunnel-certificate").is_empty());
    let on443 = format!("web: {{ https: 443 }}\n{}", ws("443", HOST));
    assert_eq!(found(&on443, "tunnel-certificate").len(), 1);
}
