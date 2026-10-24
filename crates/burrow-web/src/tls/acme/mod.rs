pub mod account;
pub mod challenge;
mod group;
pub mod guard;
pub mod http;
mod job;
mod learned;
mod manager;
pub mod order;
pub mod schedule;
pub mod storage;

pub use manager::{Acme, GroupView, Handle, Plan};

use crate::tls::CertCheck;

pub const LETSENCRYPT_PRODUCTION: &str = "https://acme-v02.api.letsencrypt.org/directory";
pub const LETSENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

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

pub fn checks_of(settings: &crate::AcmeSettings, name: &str) -> Result<Vec<CertCheck>, String> {
    use burrow_syntax::web::{Check, NoCheck, checks_for};
    match checks_for(name, settings.check_ports, settings.dns, false) {
        Ok(list) if list.contains(&Check::Dns) => Err(format!(
            "\"{name}\" needs a DNS check (DNS checks come in a later phase)"
        )),
        Ok(list) => Ok(list
            .into_iter()
            .map(|c| match c {
                Check::Http => CertCheck::Http,
                _ => CertCheck::Tls,
            })
            .collect()),
        Err(NoCheck::NoPort) => Err(format!(
            "Let's Encrypt cannot check \"{name}\": it connects only to port 443 (web.https) or port 80 (web.http)"
        )),
        Err(NoCheck::DnsForIp) => Err(format!(
            "\"{name}\" is an IP address, which Let's Encrypt checks only over TLS or HTTP"
        )),
        Err(NoCheck::DnsNotSet) => Err(format!(
            "\"{name}\" needs a DNS check (DNS checks come in a later phase)"
        )),
    }
}

pub fn checks_for_all(settings: &crate::AcmeSettings, names: &[String]) -> Vec<CertCheck> {
    let each: Vec<Vec<CertCheck>> = names
        .iter()
        .filter_map(|n| checks_of(settings, n).ok())
        .collect();
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
            dns: false,
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
        assert_eq!(checks_of(&both, "a.example.com"), Ok(vec![Tls, Http]));
        assert_eq!(
            checks_of(&le(Some(8443), Some(80)), "a.example.com"),
            Ok(vec![Http])
        );
        assert_eq!(checks_of(&both, "203.0.113.10"), Ok(vec![Tls, Http]));
        let off = checks_of(&le(Some(8443), None), "a.example.com").unwrap_err();
        assert!(off.contains("port 443") && off.contains("port 80"), "{off}");
        let wild = checks_of(&both, "*.example.com").unwrap_err();
        assert!(
            wild.ends_with("(DNS checks come in a later phase)"),
            "{wild}"
        );
        let names = vec!["a.example.com".to_string(), "b.example.com".to_string()];
        assert_eq!(checks_for_all(&both, &names), vec![Tls, Http]);
        assert_eq!(checks_for_all(&le(None, Some(80)), &names), vec![Http]);
    }
}
