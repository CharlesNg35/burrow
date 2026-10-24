use std::path::Path;

use super::tests::{SECRET, errors, settings};
use crate::config::load::load_str_in;
use crate::config::{Format, Outcome, Role};

fn load(text: &str) -> Outcome {
    let env = |name: &str| match name {
        "HOME_SECRET" => Some(SECRET.to_string()),
        "OFFICE_SECRET" => Some("0uAEjsD0O5lWx7ngXPdRMpdMvjrVqA5IFw1MQf2EYxA=".to_string()),
        "CF_TOKEN" | "TSIG" => Some("c2VjcmV0".to_string()),
        _ => None,
    };
    load_str_in(
        Path::new("."),
        "server.yaml",
        text,
        Format::Yaml,
        Some(Role::Server),
        &env,
    )
}

const EXAMPLE_1: &str = r#"# server.yaml
listen: 7835 # clients connect here (or websocket:)
clients:
  home:
    secret: ${HOME_SECRET}
web:
  https: 443
  http: 80
  letsencrypt:
    email: you@example.com
services:
  shop: { client: home, target: 8081, web: shop.example.com } # its own certificate (TLS or HTTP check)
"#;

const EXAMPLE_2: &str = r#"# server.yaml
listen: 7835
clients:
  home:
    secret: ${HOME_SECRET}
  office:
    secret: ${OFFICE_SECRET}
web:
  https: 443
  letsencrypt:
    email: you@example.com
    dns:
      - domains: [example.com, example.net] # Cloudflare manages these and every name under them
        cloudflare: { token: "${CF_TOKEN}" } # quoted: ${...} inside { } must be a quoted string in YAML
        wildcards: ["*.example.com"] # one shared certificate: example.com + *.example.com
      - domains: [apps.example.com] # a delegated subdomain: the most specific domain wins
        rfc2136: { server: 10.0.0.53:53, key_name: acme, secret: "${TSIG}" }
        wildcards: ["*.apps.example.com"] # on the entry that manages apps.example.com
  certificates:
    - hosts: ["*.corp.example.com"] # own file: hosts, cert and key all required
      cert: certs/corp-wildcard.pem
      key: certs/corp-wildcard.key
services:
  wiki: { client: home, target: 3001, web: wiki.example.com } # reuses *.example.com, orders nothing
  blog: { client: home, target: 8080, web: blog.example.com } # reuses *.example.com
  board: { client: office, target: 3000, web: board.apps.example.com } # reuses *.apps.example.com
  erp: { client: office, target: 8443, web: erp.corp.example.com } # uses the own file
  lab: # its own certificate, through DNS
    client: home
    target: 9000
    web:
      hosts: [lab.example.net]
      challenge: dns # example.net → Cloudflare, chosen by domain
"#;

const EXAMPLE_3: &str = r#"# server.yaml
clients:
  home:
    secret: ${HOME_SECRET}
web:
  https: 8443
  letsencrypt:
    email: you@example.com
    dns:
      - domains: [example.com]
        cloudflare: { token: "${CF_TOKEN}" }
websocket: # the only way in for clients here, on the shared HTTPS port
  listen: 8443
  host: tunnel.example.com
  path: /burrow
  challenge: dns # the tunnel's own name through DNS; clients use wss://tunnel.example.com:8443/burrow
"#;

#[test]
fn example_1_of_section_3_15_validates_and_says_how_the_site_is_served() {
    let o = load(EXAMPLE_1);
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
    let s = settings(o);
    let sum = crate::config::web_summary::summary(&s).unwrap();
    assert_eq!(
        sum.lines(),
        [
            "web: https [::]:443 (IPv4 and IPv6), http [::]:80 (IPv4 and IPv6)",
            "certificates: Let's Encrypt (production) as you@example.com",
            "shop.example.com: Let's Encrypt, TLS check -> service shop",
            "http [::]:80: other HTTPS hosts redirect to https:// (308); Let's Encrypt HTTP checks answered",
        ]
    );
}

fn codes(text: &str) -> Vec<(String, String)> {
    load(text)
        .report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| (d.code.to_string(), d.message.clone()))
        .collect()
}

#[test]
fn examples_2_and_3_wait_for_dns_checks_until_p6_flips_them_to_clean() {
    let pinned = |t: &str| codes(t);
    let pair = |c: &str, m: &str| (c.to_string(), m.to_string());
    assert_eq!(
        pinned(EXAMPLE_2),
        [
            pair("unknown-field", "unknown field \"dns\" in \"letsencrypt\""),
            pair(
                "challenge-dns",
                "services.lab.web.challenge: dns checks \"lab.example.net\" through a DNS record; DNS checks come in a later phase",
            ),
        ],
        "P6 adds these keys and turns example 2 into a clean fixture"
    );
    assert_eq!(
        pinned(EXAMPLE_3),
        [
            pair("unknown-field", "unknown field \"dns\" in \"letsencrypt\""),
            pair(
                "challenge-dns",
                "websocket.challenge: dns checks \"tunnel.example.com\" through a DNS record; DNS checks come in a later phase",
            ),
        ],
        "P6 adds these keys and turns example 3 into a clean fixture"
    );
}

#[test]
fn a_certificate_entry_needs_hosts_cert_and_key() {
    let text =
        "listen: 7835\nweb:\n  https: 443\n  certificates:\n    - hosts: [\"*.example.com\"]\n";
    assert_eq!(
        codes(text),
        [(
            "certificate-entry".to_string(),
            "web.certificates[0] needs hosts, cert and key: hosts names which names use the file, cert and key are the files".to_string()
        )]
    );
}
