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

fn site(web: &str, extra: &str) -> String {
    format!(
        "web:\n  https: 443\n{extra}services:\n  blog:\n    client: home\n    target: 80\n    web: {web}\n"
    )
}

const LE: &str = "  letsencrypt: {email: ops@example.com}\n";

#[test]
fn a_host_with_no_certificate_source_is_refused_with_the_ways_to_fix_it() {
    let e = found(&site("blog.example.com", ""), "no-certificate");
    assert_eq!(
        e,
        [
            "host \"blog.example.com\" (service \"blog\") has no certificate; add web.letsencrypt for automatic certificates, tls: {cert, key} on service \"blog\", or an entry in web.certificates whose hosts lists it"
        ]
    );
    let e = found(
        &site(
            "{hosts: [blog.example.com, www.example.com], tls: auto}",
            "",
        ),
        "no-certificate",
    );
    assert_eq!(e.len(), 2, "{e:#?}");
}

#[test]
fn letsencrypt_a_listing_file_or_the_sites_own_file_is_a_source() {
    assert!(found(&site("blog.example.com", LE), "no-certificate").is_empty());
    let shared = "  certificates:\n    - {hosts: [BLOG.example.com.], cert: c.pem, key: c.key}\n";
    assert!(found(&site("blog.example.com", shared), "no-certificate").is_empty());
    let own = "{hosts: [blog.example.com], tls: {cert: c.pem, key: c.key}}";
    assert!(found(&site(own, ""), "no-certificate").is_empty());
    let pass = "{hosts: [blog.example.com], tls: passthrough}";
    assert!(found(&site(pass, ""), "no-certificate").is_empty());
    let plain = "{hosts: [blog.example.com], tls: off}";
    assert!(found(&site(plain, ""), "no-certificate").is_empty());
}

#[test]
fn a_file_is_used_only_for_the_names_its_hosts_lists() {
    let wild = "  certificates:\n    - {hosts: [\"*.example.com\"], cert: c.pem, key: c.key}\n";
    let e = found(&site("blog.example.com", wild), "no-certificate");
    assert_eq!(
        e.len(),
        1,
        "a wildcard entry does not list blog.example.com"
    );
    assert!(found(&site("\"*.example.com\"", wild), "no-certificate").is_empty());
}

#[test]
fn a_wildcard_from_lets_encrypt_needs_dns_checks_this_build_does_not_have() {
    let e = found(&site("\"*.apps.example.com\"", LE), "wildcard-needs-dns");
    assert_eq!(
        e,
        [
            "services.blog.web.hosts: \"*.apps.example.com\" needs DNS-01 (Let's Encrypt gives wildcard certificates only through DNS checks), which this build does not have yet; add a certificate file under web.certificates or tls"
        ]
    );
    assert!(found(&site("\"*.apps.example.com\"", LE), "no-certificate").is_empty());
}

#[test]
fn an_ip_address_from_lets_encrypt_is_a_source() {
    assert!(found(&site("203.0.113.10", LE), "no-certificate").is_empty());
    assert!(found(&site("203.0.113.10", LE), "wildcard-needs-dns").is_empty());
}

fn tunnel(extra: &str) -> String {
    format!("web:\n  https: 443\n{extra}websocket:\n  listen: 443\n  host: tunnel.example.com\n")
}

#[test]
fn the_tunnels_name_takes_its_file_a_listing_entry_or_lets_encrypt() {
    let e = found(&tunnel(""), "tunnel-certificate");
    assert_eq!(
        e,
        [
            "websocket.host \"tunnel.example.com\" has no certificate; add web.letsencrypt, websocket.tls, or an entry in web.certificates whose hosts lists it"
        ]
    );
    assert!(found(&tunnel(LE), "tunnel-certificate").is_empty());
    let shared = "  certificates:\n    - {hosts: [tunnel.example.com], cert: c.pem, key: c.key}\n";
    assert!(found(&tunnel(shared), "tunnel-certificate").is_empty());
}
