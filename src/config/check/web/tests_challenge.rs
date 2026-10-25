use super::tests::load;

fn errors(rest: &str) -> Vec<(String, String)> {
    load(rest)
        .report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| (d.code.to_string(), d.message.clone()))
        .collect()
}

fn site(ports: &str, hosts: &str, challenge: &str) -> String {
    format!(
        "listen: 7835\nweb:\n  {ports}\n  letsencrypt: {{ email: you@example.com }}\nservices:\n  wiki:\n    client: home\n    target: 3000\n    web: {{ hosts: [{hosts}]{challenge} }}\n"
    )
}

const ONLY_443: &str = "https: 443";

#[test]
fn with_only_443_and_no_dns_the_tls_check_is_enough() {
    assert!(errors(&site(ONLY_443, "wiki.example.com", "")).is_empty());
    assert!(errors(&site(ONLY_443, "wiki.example.com", ", challenge: tls")).is_empty());
    assert_eq!(
        errors(&site(ONLY_443, "wiki.example.com", ", challenge: http")),
        [(
            "challenge-port".to_string(),
            "services.wiki.web.challenge: http needs web.http on port 80: Let's Encrypt fetches a file there; add http: 80, or use challenge: tls or dns".to_string()
        )]
    );
    assert_eq!(
        errors(&site(ONLY_443, "wiki.example.com", ", challenge: dns")),
        [(
            "challenge-dns".to_string(),
            "services.wiki.web.hosts: \"wiki.example.com\" needs DNS-01 (challenge: dns): add web.letsencrypt.dns, or use challenge: http (port 80) or tls (port 443), or give a certificate file".to_string()
        )]
    );
    let wild = errors(&site(ONLY_443, "\"*.example.com\"", ""));
    assert_eq!(wild.len(), 1, "{wild:#?}");
    assert_eq!(wild[0].0, "wildcard-needs-dns");
}

#[test]
fn with_neither_443_nor_80_no_name_can_be_checked() {
    let e = errors(&site(
        "https: 8443",
        "wiki.example.com, docs.example.com",
        "",
    ));
    assert_eq!(
        e,
        [
            (
                "no-check".to_string(),
                "wiki.example.com (service \"wiki\") cannot be checked by Let's Encrypt: open http: 80, add challenge: dns, or list a file for it in web.certificates".to_string()
            ),
            (
                "no-check".to_string(),
                "docs.example.com (service \"wiki\") cannot be checked by Let's Encrypt: open http: 80, add challenge: dns, or list a file for it in web.certificates".to_string()
            ),
        ]
    );
    assert!(errors(&site("https: 8443\n  http: 80", "wiki.example.com", "")).is_empty());
    assert_eq!(
        errors(&site(
            "https: 8443\n  http: 80",
            "wiki.example.com",
            ", challenge: tls"
        ))[0]
            .0,
        "challenge-port"
    );
}

#[test]
fn a_check_never_fits_a_wildcard_unless_it_is_dns_nor_dns_an_ip() {
    let e = errors(&site(ONLY_443, "\"*.example.com\"", ", challenge: tls"));
    assert_eq!(e[0].0, "challenge-wildcard");
    let ip = errors(&site(ONLY_443, "203.0.113.10", ", challenge: dns"));
    assert_eq!(
        ip,
        [(
            "challenge-dns".to_string(),
            "services.wiki.web.challenge: \"203.0.113.10\" is an IP address; Let's Encrypt checks IP addresses only over HTTP or TLS, not DNS".to_string()
        )]
    );
}

#[test]
fn a_challenge_where_no_certificate_is_ordered_is_refused() {
    for (tls, why) in [
        (
            "passthrough",
            "the app keeps its own certificate (tls: passthrough)",
        ),
        ("off", "a site with tls: off has no HTTPS"),
    ] {
        let text = site(
            "https: 443\n  http: 80",
            "wiki.example.com",
            &format!(", tls: {tls}, challenge: http"),
        );
        assert_eq!(
            errors(&text),
            [(
                "challenge-unused".to_string(),
                format!("services.wiki.web.challenge: {why}; remove challenge")
            )]
        );
    }
}

#[test]
fn the_tunnels_own_name_takes_a_challenge_too() {
    let tunnel = |ports: &str, extra: &str| {
        format!(
            "web:\n  {ports}\n  letsencrypt: {{ email: you@example.com }}\nwebsocket:\n  listen: 443\n  host: tunnel.example.com\n{extra}"
        )
    };
    assert!(errors(&tunnel(ONLY_443, "  challenge: tls\n")).is_empty());
    assert_eq!(
        errors(&tunnel(ONLY_443, "  challenge: http\n"))[0].1,
        "websocket.challenge: http needs web.http on port 80: Let's Encrypt fetches a file there; add http: 80, or use challenge: tls or dns"
    );
    let unshared = "listen: 7835\nwebsocket:\n  listen: 7836\n  challenge: tls\n";
    assert!(
        errors(unshared).is_empty(),
        "no host: validate says nothing more than before"
    );
}
