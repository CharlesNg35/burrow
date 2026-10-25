use super::tests::{SECRET, files_for, paths_after};
use crate::config::load::load_str_in;
use crate::config::{Format, Outcome, Role};

const WEB_MD: &str = include_str!("../../../../docs/web.md");
const WEB_PAGES: &[(&str, &str)] = &[
    ("web.md", WEB_MD),
    (
        "web/certificates.md",
        include_str!("../../../../docs/web/certificates.md"),
    ),
    (
        "web/routing.md",
        include_str!("../../../../docs/web/routing.md"),
    ),
    (
        "web/headers-limits-pages.md",
        include_str!("../../../../docs/web/headers-limits-pages.md"),
    ),
    (
        "web/proxies.md",
        include_str!("../../../../docs/web/proxies.md"),
    ),
    (
        "web/http2-grpc.md",
        include_str!("../../../../docs/web/http2-grpc.md"),
    ),
    (
        "web/health-checks.md",
        include_str!("../../../../docs/web/health-checks.md"),
    ),
    ("web/logs.md", include_str!("../../../../docs/web/logs.md")),
    (
        "web/recipes.md",
        include_str!("../../../../docs/web/recipes.md"),
    ),
    (
        "web/tunnel.md",
        include_str!("../../../../docs/web/tunnel.md"),
    ),
    (
        "web/full-example.md",
        include_str!("../../../../docs/web/full-example.md"),
    ),
];
const WEBSOCKET_MD: &str = include_str!("../../../../docs/websocket.md");

const OLD_HEADINGS: &[&str] = &[
    "A first website",
    "The ports",
    "Port forwarding at home",
    "How a website is served",
    "What burrow does for a website",
    "Certificates",
    "Your own files",
    "Let's Encrypt",
    "DNS checks: wildcards and names the internet cannot reach",
    "Shared wildcards",
    "Cloudflare",
    "RFC 2136: BIND, Knot, PowerDNS, Technitium",
    "acme-dns",
    "A command for any provider",
    "When a certificate fails",
    "Plain HTTP websites",
    "Passthrough",
    "Paths: several apps on one name",
    "How burrow reads a path",
    "Redirects",
    "Headers, limits and pages",
    "HTTPS to the app",
    "Behind Cloudflare or a load balancer",
    "HTTP/2",
    "gRPC",
    "gRPC streams",
    "Health checks for websites",
    "Access log and request ids",
    "Examples",
    "The minimum: websites with automatic HTTPS",
    "One site with two names",
    "Your own certificates only (Let's Encrypt never contacted)",
    "Many sites under one domain: one shared wildcard",
    "Many apps under your own wildcard file",
    "Plain HTTP only (LAN, or behind a proxy that does HTTPS)",
    "An app on the server itself",
    "Single sign-on with authentik",
    "Home Assistant",
    "Nextcloud behind Cloudflare",
    "Grafana under a path",
    "A site for an IP address, with no domain",
    "On the client",
    "The tunnel on the same port",
    "The tunnel on port 8443, no port 80, through DNS",
    "The tunnel on port 80, plain ws (still encrypted by burrow)",
    "A full example",
    "What validate checks",
    "Limits",
];

const FIRST_SERVER: &[(&str, &str)] = &[
    ("listen:", "listen: 7835\n"),
    (
        "clients:",
        "clients:\n  home:\n    secret: ${HOME_SECRET}\n  office:\n    secret: ${OFFICE_SECRET}\n",
    ),
];
const FIRST_CLIENT: &[(&str, &str)] = &[
    ("server:", "server: tunnel.example.com:7835\n"),
    ("name:", "name: home\n"),
    ("secret:", "secret: ${HOME_SECRET}\n"),
];

fn yaml_blocks(md: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for line in md.lines() {
        let trimmed = line.trim_start();
        match open.as_mut() {
            None if trimmed == "```yaml" => {
                open = Some((line.len() - trimmed.len(), String::new()))
            }
            None => {}
            Some(_) if trimmed == "```" => blocks.push(open.take().unwrap().1),
            Some((indent, text)) => {
                text.push_str(line.get(*indent..).unwrap_or(""));
                text.push('\n');
            }
        }
    }
    blocks
}

fn env(name: &str) -> Option<String> {
    match name {
        "HOME_SECRET" => Some(SECRET.to_string()),
        "OFFICE_SECRET" => Some("0uAEjsD0O5lWx7ngXPdRMpdMvjrVqA5IFw1MQf2EYxA=".to_string()),
        "LAPTOP_SECRET" => Some("YoO7pXZ3rpxdzO0+O+KzyqPc+EzxviqjbPn7J/7S0eU=".to_string()),
        _ => Some("c2VjcmV0".to_string()),
    }
}

fn burrows(block: &str) -> bool {
    ["# server.yaml", "# client.yaml"]
        .iter()
        .any(|h| block.starts_with(h))
}

pub(super) fn complete(block: &str) -> (Role, String) {
    let (role, first) = if block.starts_with("# server.yaml") {
        (Role::Server, FIRST_SERVER)
    } else if block.starts_with("# client.yaml") {
        (Role::Client, FIRST_CLIENT)
    } else {
        panic!("a config block starts with # server.yaml or # client.yaml:\n{block}")
    };
    let missing: String = first
        .iter()
        .filter(|(key, _)| !block.lines().any(|l| l.starts_with(key)))
        .map(|(_, text)| *text)
        .collect();
    (role, format!("{missing}{block}"))
}

pub(super) fn validate(block: &str) -> Outcome {
    validate_in(block).1
}

fn validate_in(block: &str) -> (tempfile::TempDir, Outcome) {
    let (role, mut text) = complete(block);
    for path in ["cert:", "key:", "ca:"]
        .iter()
        .flat_map(|f| paths_after(&text, f))
        .collect::<Vec<_>>()
    {
        if let Some(relative) = path.strip_prefix('/') {
            text = text.replace(&path, relative);
        }
    }
    let dir = files_for(&text);
    for ca in paths_after(&text, "ca:") {
        let path = dir.path().join(ca);
        if path.exists() {
            continue;
        }
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let cert = params.self_signed(&key).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, cert.pem()).unwrap();
    }
    for folder in paths_after(&text, "error_pages:") {
        let folder = dir.path().join(folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("404.html"), "not here").unwrap();
    }
    let o = load_str_in(
        dir.path(),
        "config.yaml",
        &text,
        Format::Yaml,
        Some(role),
        &env,
    );
    (dir, o)
}

fn errors(o: &Outcome) -> Vec<String> {
    o.report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect()
}

#[test]
fn every_config_block_in_the_web_docs_validates() {
    let mut broken = Vec::new();
    let pages = WEB_PAGES.iter().chain([&("websocket.md", WEBSOCKET_MD)]);
    for (doc, md) in pages {
        let blocks: Vec<String> = yaml_blocks(md).into_iter().filter(|b| burrows(b)).collect();
        assert!(!blocks.is_empty() || !md.contains("```yaml"), "{doc}");
        for block in blocks {
            let e = errors(&validate(&block));
            if !e.is_empty() {
                broken.push(format!("{doc}:\n{block}=> {e:#?}"));
            }
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n\n"));
}

fn headings(md: &str) -> Vec<&str> {
    let mut fenced = false;
    md.lines()
        .filter(|l| {
            fenced ^= l.trim_start().starts_with("```");
            !fenced && l.starts_with('#')
        })
        .map(|l| l.trim_start_matches('#').trim())
        .collect()
}

#[test]
fn every_heading_of_the_one_page_web_md_is_still_in_the_web_docs() {
    let all: Vec<&str> = WEB_PAGES.iter().flat_map(|(_, md)| headings(md)).collect();
    let lost: Vec<&&str> = OLD_HEADINGS.iter().filter(|h| !all.contains(h)).collect();
    assert!(lost.is_empty(), "{lost:?}");
}

#[test]
fn every_page_in_docs_web_is_checked() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/web");
    let mut pages: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| format!("web/{}", e.unwrap().file_name().to_string_lossy()))
        .collect();
    pages.sort();
    let mut checked: Vec<String> = WEB_PAGES
        .iter()
        .skip(1)
        .map(|(n, _)| n.to_string())
        .collect();
    checked.sort();
    assert_eq!(pages, checked);
}

#[test]
fn the_admin_uis_lines_to_add_validate() {
    let block = format!(
        "{}services:\n  shop:\n    client: home\n    target: 8081\n    web: shop.example.com\n",
        burrow_admin_logic::web::TURN_ON
    );
    assert_eq!(errors(&validate(&block)), Vec::<String>::new());
}

#[test]
fn the_full_example_of_the_web_docs_is_pinned_at_what_p3_and_p6_build() {
    let block = WEB_PAGES
        .iter()
        .flat_map(|(_, md)| yaml_blocks(md))
        .find(|b| b.contains("1. THE TUNNEL"))
        .expect("the full example");
    let (dir, o) = validate_in(&block);
    let others: Vec<String> = o
        .report
        .diagnostics
        .iter()
        .filter(|d| !["low-port", "certificate-extra"].contains(&d.code))
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect();
    assert_eq!(
        others,
        [
            "dns-rfc2136: web.letsencrypt.dns[1].rfc2136.secret is 6 bytes; RFC 8945 asks for at least 32 for hmac-sha256",
            "proxy-tls-check: a proxy is in front (web.trusted_proxies is set); if it does the HTTPS, like Cloudflare's proxy, Let's Encrypt's TLS check cannot reach burrow: set challenge: http (with http: 80) or challenge: dns on these sites: kiosk",
            "access-log: web.access_log: the folder of logs/access.log does not exist yet",
            "target-unchecked: services.pve: the app's certificate is not checked (insecure_skip_verify); use ca: for apps with a real certificate",
            "plain-http: services.dashboard: served as plain HTTP; anyone on the path can read and change its traffic",
            "plain-http: services.wiki: served as plain HTTP; anyone on the path can read and change its traffic",
            "redirect-by-proxy: services.wiki: http:// visitors are sent to https:// by the X-Forwarded-Proto of the trusted proxies; if they do not send it, every request redirects",
        ],
        "the machine's low-port warnings and the notes for the test files aside, the notes the example's comments promise (the test's short TSIG secret aside)"
    );
    let root = dir.path().display().to_string();
    let lines: Vec<String> = crate::config::web_summary::summary(&super::tests::settings(o))
        .unwrap()
        .lines()
        .iter()
        .map(|l| {
            l.replace(&format!("{root}{}", std::path::MAIN_SEPARATOR), "")
                .replace(&format!("{root}/"), "")
        })
        .collect();
    assert_eq!(
        lines,
        [
            "web: https [::]:443 (HTTP/2 and HTTP/1.1, IPv4 and IPv6; shared with the tunnel), http [::]:80 (IPv4 and IPv6)",
            "X-Forwarded-For believed from 173.245.48.0/20",
            "certificates: Let's Encrypt (production) as you@example.com; DNS checks through Cloudflare for example.com, example.net, nameservers found via the system resolvers; DNS checks through RFC 2136 for corp.example.org, nameservers found via the system resolvers",
            "tunnel: wss://tunnel.example.com/burrow (Let's Encrypt shared wildcard *.example.com)",
            "wildcard *.example.com (+ example.com): Let's Encrypt, DNS check; used by api, blog, login, pve, shop, store, updates",
            "wildcard *.apps.example.com (+ apps.example.com): Let's Encrypt, DNS check; used by apps",
            "*.apps.example.com: Let's Encrypt shared wildcard *.apps.example.com -> service apps",
            "203.0.113.10: Let's Encrypt short-lived (6 days), TLS check -> service kiosk",
            "auth.example.com: Let's Encrypt shared wildcard *.example.com -> service login",
            "dash.lan.example.com: plain HTTP on [::]:80 only, unencrypted (tls: off) -> service dashboard",
            "erp.corp.example.org: Let's Encrypt, DNS check through RFC 2136 (corp.example.org) (challenge: dns)",
            "  /           -> service erp (HTTPS to the app, checked with certs/corp-ca.pem)",
            "example.com: Let's Encrypt shared wildcard *.example.com",
            "  /2019       -> redirect /archive/2019 (permanent)",
            "  /feed       -> redirect https://feeds.example.net/blog (temporary)",
            "  /api        -> service api (app sees /v1, X-Forwarded-Prefix /api)",
            "  /           -> service blog",
            "example.io: Let's Encrypt, TLS check",
            "  /           -> redirect https://example.com (permanent)",
            "nas.example.com: passthrough, the app's own certificate -> service nas",
            "news.example.net: Let's Encrypt, HTTP check -> service news",
            "old-blog.example.com: Let's Encrypt shared wildcard *.example.com",
            "  /           -> redirect https://example.com (temporary)",
            "partner.example.net: own file certs/partner.pem -> service partner",
            "portal.intranet.example.org: web.certificates[0] (certs/intranet-wildcard.pem) -> service portal",
            "pve.example.com: Let's Encrypt shared wildcard *.example.com",
            "  /           -> service pve (HTTPS to the app, not checked)",
            "shop.example.com: Let's Encrypt shared wildcard *.example.com -> service shop",
            "store.example.com: Let's Encrypt shared wildcard *.example.com",
            "  /           -> service store (maintenance page)",
            "updates.example.com: Let's Encrypt shared wildcard *.example.com; http:// served too (https_redirect: false) -> service updates",
            "wiki.example.net: plain HTTP on [::]:80 only, unencrypted (tls: off)",
            "  /           -> service wiki (http:// at the proxy redirects to https:// by X-Forwarded-Proto)",
            "www.example.com: Let's Encrypt shared wildcard *.example.com",
            "  /           -> redirect https://example.com (permanent)",
            "http [::]:80: HTTPS hosts redirect to https:// (308); Let's Encrypt HTTP checks answered",
        ],
        "each name says where its certificate comes from, as the list under the example does"
    );
}
