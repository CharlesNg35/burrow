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
fn a_wildcard_entry_covers_the_names_one_label_under_it() {
    let wild = "  certificates:\n    - {hosts: [\"*.example.com\"], cert: c.pem, key: c.key}\n";
    assert!(found(&site("blog.example.com", wild), "no-certificate").is_empty());
    assert!(found(&site("\"*.example.com\"", wild), "no-certificate").is_empty());
    let e = found(&site("a.b.example.com", wild), "no-certificate");
    assert_eq!(e.len(), 1, "two labels down is not covered: {e:#?}");
    assert_eq!(found(&site("example.com", wild), "no-certificate").len(), 1);
}

#[test]
fn a_name_is_listed_in_one_entry_only_and_every_entry_lists_names() {
    let twice = "  certificates:\n    - {hosts: [blog.example.com], cert: a.pem, key: a.key}\n    - {hosts: [\"*.example.com\", Blog.Example.com], cert: b.pem, key: b.key}\n";
    assert_eq!(
        found(&site("blog.example.com", twice), "certificate-hosts"),
        [
            "host \"blog.example.com\" is listed in web.certificates[0] and [1]; list it in one entry only"
        ]
    );
    let exact_and_wild = "  certificates:\n    - {hosts: [blog.example.com], cert: a.pem, key: a.key}\n    - {hosts: [\"*.example.com\"], cert: b.pem, key: b.key}\n";
    assert!(
        found(
            &site("blog.example.com", exact_and_wild),
            "certificate-hosts"
        )
        .is_empty()
    );
    let none = "  certificates:\n    - {hosts: [], cert: a.pem, key: a.key}\n";
    assert_eq!(
        found(&site("blog.example.com", none), "certificate-hosts"),
        [
            "web.certificates[0].hosts is required: the names this file is for, like hosts: [\"*.corp.example.com\"]"
        ]
    );
}

#[test]
fn the_own_certificates_example_of_section_4_7_has_a_source_for_every_name() {
    let text = r#"web:
  https: 443
  certificates: # shared files; each says which names use it
    - { hosts: ["*.example.com"], cert: certs/star.example.com.pem, key: certs/star.example.com.key }
    - { hosts: [app.home.arpa], cert: certs/home.pem, key: certs/home.key } # an internal name
services:
  intranet:
    client: office
    target: 80
    web:
      hosts: [intranet.corp.example.com]
      tls: { cert: certs/intranet.pem, key: certs/intranet.key }
  mixed:
    client: home
    target: 8080
    web:
      hosts: [app.example.com, app.home.arpa]
      # app.example.com uses the shared *.example.com file, app.home.arpa its own entry above
"#
    .replace("client: office", "client: home");
    assert!(found(&text, "no-certificate").is_empty());
    assert!(found(&text, "certificate-hosts").is_empty());
    let e = super::tests::errors(&load(&text));
    assert!(
        e.iter().all(|m| m.contains("needs \"listen\"")),
        "only the missing listen: {e:#?}"
    );
}

#[test]
fn the_shared_file_of_section_4_2_serves_erp() {
    let text = r#"web:
  https: 443
  certificates: # shared files, for names from your own CA; each says which names use it
    - hosts: ["*.corp.example.com"]
      cert: certs/wildcard.corp.example.com.pem
      key: certs/wildcard.corp.example.com.key
services:
  erp:
    client: home
    target: erp.corp.example.com:443
    web:
      hosts: [erp.corp.example.com] # served with the shared file above
"#;
    assert!(found(text, "no-certificate").is_empty());
    let e = super::tests::errors(&load(text));
    assert!(
        e.iter().all(|m| m.contains("needs \"listen\"")),
        "only the missing listen: {e:#?}"
    );
    let without = text.replace("*.corp.example.com", "*.other.example.com");
    assert_eq!(found(&without, "no-certificate").len(), 1);
}

#[test]
fn a_wildcard_from_lets_encrypt_needs_a_dns_provider() {
    let e = found(&site("\"*.apps.example.com\"", LE), "wildcard-needs-dns");
    assert_eq!(
        e,
        [
            "services.blog.web.hosts: \"*.apps.example.com\" needs DNS-01 (Let's Encrypt gives wildcard certificates only through DNS): add web.letsencrypt.dns, or a certificate file under web.certificates or tls"
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
