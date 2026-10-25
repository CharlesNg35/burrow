use super::tests::{SECRET, files_for, paths_after};
use crate::config::load::load_str_in;
use crate::config::{Format, Outcome, Role};

const WEB_MD: &str = include_str!("../../../../docs/web.md");
const WEBSOCKET_MD: &str = include_str!("../../../../docs/websocket.md");

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
    for (doc, md) in [("web.md", WEB_MD), ("websocket.md", WEBSOCKET_MD)] {
        let blocks: Vec<String> = yaml_blocks(md).into_iter().filter(|b| burrows(b)).collect();
        assert!(blocks.len() > 5, "{doc}");
        for block in blocks {
            let e = errors(&validate(&block));
            if !e.is_empty() {
                broken.push(format!("{doc}:\n{block}=> {e:#?}"));
            }
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n\n"));
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
fn the_full_example_of_web_md_is_pinned_at_what_p2_builds() {
    let block = yaml_blocks(WEB_MD)
        .into_iter()
        .find(|b| b.contains("1. THE TUNNEL"))
        .expect("the full example");
    let (dir, o) = validate_in(&block);
    let others: Vec<String> = o
        .report
        .diagnostics
        .iter()
        .filter(|d| d.code != "low-port")
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect();
    assert_eq!(
        others,
        Vec::<String>::new(),
        "only the machine's low-port warnings"
    );
    let at = format!("{}/", dir.path().display());
    let lines: Vec<String> = crate::config::web_summary::summary(&super::tests::settings(o))
        .unwrap()
        .lines()
        .iter()
        .map(|l| l.replace(&at, ""))
        .collect();
    assert_eq!(
        lines,
        [
            "web: https [::]:443 (IPv4 and IPv6; shared with the tunnel), http [::]:80 (IPv4 and IPv6)",
            "certificates: Let's Encrypt (production) as you@example.com",
            "tunnel: wss://tunnel.example.com/burrow (Let's Encrypt, TLS check)",
            "203.0.113.10: Let's Encrypt short-lived (6 days), TLS check -> service kiosk",
            "auth.example.com: Let's Encrypt, TLS check -> service login",
            "dash.lan.example.com: plain HTTP on [::]:80 only, unencrypted (tls: off) -> service dashboard",
            "example.com: Let's Encrypt, TLS check -> service blog",
            "nas.example.com: passthrough, the app's own certificate -> service nas",
            "news.example.net: Let's Encrypt, HTTP check -> service news",
            "partner.example.net: own file certs/partner.pem -> service partner",
            "portal.intranet.example.org: web.certificates[0] (certs/intranet-wildcard.pem) -> service portal",
            "shop.example.com: Let's Encrypt, TLS check -> service shop",
            "updates.example.com: Let's Encrypt, TLS check; http:// served too (https_redirect: false) -> service updates",
            "www.example.com: Let's Encrypt, TLS check -> service blog",
            "http [::]:80: HTTPS hosts redirect to https:// (308); Let's Encrypt HTTP checks answered",
        ],
        "P3 adds trusted_proxies and its note for kiosk, P6 the DNS wildcards"
    );
}
