pub mod account;
pub mod challenge;
pub mod http;
mod job;
mod manager;
pub mod order;
pub mod schedule;
pub mod storage;

pub use manager::{Acme, GroupView, Handle, Plan};

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
}
