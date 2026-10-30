pub mod account;
pub mod challenge;
pub mod dns01;
mod group;
pub mod guard;
pub mod http;
mod job;
mod learned;
mod manager;
pub mod order;
pub mod schedule;
pub mod storage;

pub use job::PROFILE_NOT_OFFERED;
pub use manager::{Acme, GroupView, Handle, Plan};

use crate::tls::CertCheck;

pub use burrow_syntax::web::{LETSENCRYPT_PRODUCTION, LETSENCRYPT_STAGING};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problemish {
    RateLimited,
    AlreadyReplaced,
    Unsupported,
    Other,
}

impl Problemish {
    pub fn of(e: &instant_acme::Error) -> Problemish {
        match e {
            instant_acme::Error::Api(p) => match p.r#type.as_deref().map(short_type) {
                Some("rateLimited") => Problemish::RateLimited,
                Some("alreadyReplaced") => Problemish::AlreadyReplaced,
                _ => Problemish::Other,
            },
            instant_acme::Error::Unsupported(_) => Problemish::Unsupported,
            _ => Problemish::Other,
        }
    }
}

fn short_type(t: &str) -> &str {
    t.strip_prefix("urn:ietf:params:acme:error:").unwrap_or(t)
}

pub fn checks_of(
    settings: &crate::AcmeSettings,
    name: &str,
    choice: Option<burrow_syntax::web::Challenge>,
) -> Result<Vec<CertCheck>, String> {
    use burrow_syntax::web::{Check, NoCheck, checks_for};
    let dns = dns01::entry_for(&settings.dns, name).is_some();
    match checks_for(name, settings.check_ports, dns, choice) {
        Ok(list) => Ok(list
            .into_iter()
            .map(|c| match c {
                Check::Dns => CertCheck::Dns,
                Check::Http => CertCheck::Http,
                Check::Tls => CertCheck::Tls,
            })
            .collect()),
        Err(NoCheck::NoPort) => Err(format!(
            "{name} cannot be checked by Let's Encrypt: open http: 80, list its domain in web.letsencrypt.dns, or list a file for it in web.certificates"
        )),
        Err(NoCheck::DnsForIp) => Err(format!(
            "\"{name}\" is an IP address; Let's Encrypt checks IP addresses only over HTTP or TLS, not DNS"
        )),
        Err(NoCheck::DnsNotSet) => Err(format!(
            "\"{name}\" needs a DNS check, but no web.letsencrypt.dns entry lists its domain"
        )),
        Err(NoCheck::NoHttpPort) => Err(format!(
            "{name}: http needs web.http on port 80: Let's Encrypt fetches a file there; add http: 80, or use challenge: tls or dns"
        )),
        Err(NoCheck::NoTlsPort) => Err(format!(
            "{name}: tls needs web.https on port 443: Let's Encrypt connects there; use https: 443, or use challenge: http or dns"
        )),
        Err(NoCheck::WildcardNotDns) => Err(format!(
            "\"{name}\" is a wildcard, and Let's Encrypt gives wildcards only through DNS checks; use challenge: dns, or give a wildcard file in web.certificates"
        )),
    }
}

pub fn dns_names(
    settings: &crate::AcmeSettings,
    names: &[String],
    choice: Option<burrow_syntax::web::Challenge>,
) -> Vec<String> {
    names
        .iter()
        .filter(|n| checks_of(settings, n, choice).is_ok_and(|c| c == [CertCheck::Dns]))
        .cloned()
        .collect()
}

pub fn checks_for_all(
    settings: &crate::AcmeSettings,
    names: &[String],
    choice: Option<burrow_syntax::web::Challenge>,
) -> Vec<CertCheck> {
    let each: Vec<Vec<CertCheck>> = names
        .iter()
        .filter_map(|n| checks_of(settings, n, choice).ok())
        .filter(|l| l.as_slice() != [CertCheck::Dns])
        .collect();
    if each.is_empty() {
        return match dns_names(settings, names, choice).is_empty() {
            true => Vec::new(),
            false => vec![CertCheck::Dns],
        };
    }
    [CertCheck::Tls, CertCheck::Http]
        .into_iter()
        .filter(|c| each.iter().all(|l| l.contains(c)))
        .collect()
}

pub fn hint(name: &str, p: &instant_acme::Problem) -> Option<String> {
    let detail = p.detail.as_deref().unwrap_or_default().to_ascii_lowercase();
    let kind = p.r#type.as_deref().map(short_type);
    let in_front = match kind {
        Some("tls") => true,
        Some("connection" | "dns") => false,
        _ => detail.contains("alpn") && detail.contains("acme-tls/1"),
    };
    if in_front {
        return Some(format!(
            "something in front of this server answers HTTPS for {name} (a CDN like Cloudflare?); open http: 80 and use challenge: http, or use challenge: dns"
        ));
    }
    let unreachable = match kind {
        Some("connection" | "dns") => true,
        _ => [
            "timeout",
            "connection refused",
            "no valid a",
            "nxdomain",
            "no route",
        ]
        .iter()
        .any(|w| detail.contains(w)),
    };
    unreachable.then(|| {
        format!(
            "{name} may not be reachable from the internet; for internal names use challenge: dns"
        )
    })
}

pub fn problem_text(p: &instant_acme::Problem) -> String {
    let mut out = p.detail.clone().unwrap_or_else(|| "the CA refused".into());
    if let Some(t) = &p.r#type {
        out.push_str(&format!(" ({})", short_type(t)));
    }
    for s in &p.subproblems {
        if let Some(d) = &s.detail {
            out.push_str(&format!("; {d}"));
        }
    }
    out
}

pub fn describe(e: &instant_acme::Error) -> String {
    match e {
        instant_acme::Error::Api(p) => problem_text(p),
        other => other.to_string(),
    }
}

pub fn fraction() -> f64 {
    let mut b = [0u8; 8];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b);
    (u64::from_le_bytes(b) >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problems_read_as_their_detail_and_short_type() {
        let p: instant_acme::Problem = serde_json::from_value(serde_json::json!({
            "type": "urn:ietf:params:acme:error:rateLimited",
            "detail": "too many certificates (5) already issued for this exact set of identifiers",
            "status": 429,
            "subproblems": [{"type": "urn:ietf:params:acme:error:caa", "detail": "CAA forbids issuance",
                "identifier": {"type": "dns", "value": "a.example.com"}}]
        }))
        .unwrap();
        assert_eq!(
            problem_text(&p),
            "too many certificates (5) already issued for this exact set of identifiers (rateLimited); CAA forbids issuance"
        );
        assert_eq!(
            Problemish::of(&instant_acme::Error::Api(p)),
            Problemish::RateLimited
        );
        let f = fraction();
        assert!((0.0..1.0).contains(&f));
    }

    fn le(https: Option<u16>, http: Option<u16>) -> crate::AcmeSettings {
        crate::AcmeSettings {
            directory_url: LETSENCRYPT_PRODUCTION.into(),
            ca: None,
            email: "ops@example.com".into(),
            profile: None,
            state_dir: None,
            check_ports: burrow_syntax::web::CheckPorts { https, http },
            dns: Vec::new(),
            fallback_directory: None,
        }
    }

    fn problem(kind: &str, detail: &str) -> instant_acme::Problem {
        serde_json::from_value(serde_json::json!({
            "type": format!("urn:ietf:params:acme:error:{kind}"), "detail": detail, "status": 400
        }))
        .unwrap()
    }

    #[test]
    fn a_check_blocked_in_front_or_an_unreachable_name_gets_its_hint() {
        let alpn = problem(
            "tls",
            "Cannot negotiate ALPN protocol \"acme-tls/1\" for tls-alpn-01 challenge",
        );
        assert_eq!(
            hint("wiki.example.com", &alpn).unwrap(),
            "something in front of this server answers HTTPS for wiki.example.com (a CDN like Cloudflare?); open http: 80 and use challenge: http, or use challenge: dns"
        );
        for p in [
            problem("connection", "127.0.0.1: Connection refused"),
            problem("dns", "DNS problem: NXDOMAIN looking up A for nas.lan"),
            problem(
                "unauthorized",
                "Fetching http://x/.well-known/acme-challenge/t: Timeout during connect",
            ),
        ] {
            assert_eq!(
                hint("nas.lan.example.com", &p).unwrap(),
                "nas.lan.example.com may not be reachable from the internet; for internal names use challenge: dns"
            );
        }
        assert_eq!(
            hint(
                "a.example.com",
                &problem(
                    "unauthorized",
                    "The key authorization file from the server did not match"
                )
            ),
            None
        );
    }

    #[test]
    fn the_runtime_takes_each_names_checks_from_the_shared_rule() {
        use crate::tls::CertCheck::{Http, Tls};
        let both = le(Some(443), Some(80));
        assert_eq!(checks_of(&both, "a.example.com", None), Ok(vec![Tls, Http]));
        assert_eq!(
            checks_of(&le(Some(8443), Some(80)), "a.example.com", None),
            Ok(vec![Http])
        );
        assert_eq!(checks_of(&both, "203.0.113.10", None), Ok(vec![Tls, Http]));
        let off = checks_of(&le(Some(8443), None), "a.example.com", None).unwrap_err();
        assert!(off.contains("open http: 80"), "{off}");
        use burrow_syntax::web::Challenge;
        assert_eq!(
            checks_of(&both, "a.example.com", Some(Challenge::Http)),
            Ok(vec![Http])
        );
        assert_eq!(
            checks_of(&both, "a.example.com", Some(Challenge::Tls)),
            Ok(vec![Tls])
        );
        let no80 =
            checks_of(&le(Some(443), None), "a.example.com", Some(Challenge::Http)).unwrap_err();
        assert!(no80.contains("http needs web.http on port 80"), "{no80}");
        let wild = checks_of(&both, "*.example.com", None).unwrap_err();
        assert_eq!(
            wild,
            "\"*.example.com\" needs a DNS check, but no web.letsencrypt.dns entry lists its domain"
        );
        let names = vec!["a.example.com".to_string(), "b.example.com".to_string()];
        assert_eq!(checks_for_all(&both, &names, None), vec![Tls, Http]);
        assert_eq!(
            checks_for_all(&le(None, Some(80)), &names, None),
            vec![Http]
        );
        assert_eq!(
            checks_for_all(&both, &names, Some(burrow_syntax::web::Challenge::Http)),
            vec![Http]
        );
    }

    #[test]
    fn dns_names_are_checked_over_dns_and_the_rest_of_the_group_keeps_its_own_check() {
        use crate::tls::CertCheck::{Dns, Http, Tls};
        let mut both = le(Some(443), Some(80));
        both.dns = vec![dns01::DnsEntry {
            domains: vec!["example.com".into()],
            provider: dns01::DnsProvider::Command {
                path: "/usr/local/bin/burrow-dns".into(),
            },
            wait: dns01::DNS_WAIT,
            resolvers: Vec::new(),
            wildcards: Vec::new(),
        }];
        assert_eq!(checks_of(&both, "*.example.com", None), Ok(vec![Dns]));
        use burrow_syntax::web::Challenge;
        assert_eq!(
            checks_of(&both, "lab.example.com", Some(Challenge::Dns)),
            Ok(vec![Dns])
        );
        let mixed = vec!["example.com".to_string(), "*.example.com".to_string()];
        assert_eq!(checks_for_all(&both, &mixed, None), vec![Tls, Http]);
        assert_eq!(dns_names(&both, &mixed, None), vec!["*.example.com"]);
        let wild = vec!["*.example.com".to_string()];
        assert_eq!(checks_for_all(&both, &wild, None), vec![Dns]);
        let outside = vec!["*.example.org".to_string()];
        assert!(checks_for_all(&both, &outside, None).is_empty());
    }
}
