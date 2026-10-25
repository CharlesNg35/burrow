use std::time::Duration;

use crate::config::settings::{DnsProvider, SecretText};
use burrow_config::DnsEntry;
use serde_json::json;

fn entries(
    list: serde_json::Value,
    dir: &std::path::Path,
) -> Vec<crate::config::settings::DnsSettings> {
    let list: Vec<DnsEntry> = serde_json::from_value(list).unwrap();
    super::fill_dns::entries(&list, dir)
}

#[test]
fn a_dns_entry_reaches_the_settings_with_its_defaults_and_its_secret_from_a_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tsig.secret"), "c2VjcmV0LXNlY3JldA==\n").unwrap();
    let got = entries(
        json!([
            {"domains": ["Example.com", "example.net."], "cloudflare": {"token": "cf-token"},
             "wildcards": ["*.Example.com"]},
            {"domains": ["corp.example.org"], "wait_secs": 30, "resolvers": ["1.1.1.1", "[2606:4700::1111]:5353"],
             "rfc2136": {"server": "10.0.0.53:53", "key_name": "acme", "secret": "file:tsig.secret"}},
            {"domains": ["example.dev"], "command": "hooks/burrow-dns"}
        ]),
        dir.path(),
    );
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].domains, ["example.com", "example.net"]);
    assert_eq!(got[0].wildcards, ["*.example.com"]);
    assert_eq!(got[0].wait, Duration::from_secs(120));
    assert!(got[0].resolvers.is_empty(), "empty means the system's");
    assert_eq!(
        got[0].provider,
        DnsProvider::Cloudflare {
            token: SecretText::new("cf-token".into())
        }
    );
    assert_eq!(got[1].wait, Duration::from_secs(30));
    assert_eq!(
        got[1].resolvers,
        [
            "1.1.1.1:53".parse().unwrap(),
            "[2606:4700::1111]:5353".parse().unwrap()
        ]
    );
    match &got[1].provider {
        DnsProvider::Rfc2136 {
            server,
            key_name,
            secret,
        } => {
            assert_eq!(
                (server.as_str(), key_name.as_str()),
                ("10.0.0.53:53", "acme")
            );
            assert_eq!(secret.expose(), "c2VjcmV0LXNlY3JldA==");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        got[2].provider,
        DnsProvider::Command {
            path: dir.path().join("hooks/burrow-dns")
        }
    );
    let shown = format!("{got:?}");
    assert!(
        !shown.contains("cf-token") && !shown.contains("c2VjcmV0"),
        "{shown}"
    );
}

#[test]
fn an_entry_without_exactly_one_provider_or_with_an_unreadable_or_empty_secret_is_left_out() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("empty.token"), "\n").unwrap();
    let got = entries(
        json!([
            {"domains": ["a.example"]},
            {"domains": ["b.example"], "cloudflare": {"token": "t"}, "command": "x"},
            {"domains": ["c.example"], "cloudflare": {"token": "file:missing.token"}},
            {"domains": ["d.example"], "cloudflare": {"token": "file:empty.token"}},
            {"domains": ["e.example"], "cloudflare": {"token": "  "}}
        ]),
        dir.path(),
    );
    assert!(got.is_empty(), "{got:?}");
}
