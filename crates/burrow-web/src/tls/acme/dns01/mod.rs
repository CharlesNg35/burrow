use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use hickory_proto::rr::Name;
use zeroize::Zeroizing;

pub mod acme_dns;
pub mod cloudflare;
pub mod command;
mod fake;
pub mod lookup;
mod mock_api;
pub mod pending;
pub mod propagate;
pub mod rfc2136;
pub mod system;
pub mod wire;

pub const DEFAULT_WAIT: Duration = Duration::from_secs(120);
pub const TXT_TTL: u32 = 60;

pub type Fut<'a> = Pin<Box<dyn Future<Output = Result<(), Refusal>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    NotWritable(String),
    Failed(String),
}

impl Refusal {
    pub fn text(&self) -> &str {
        match self {
            Refusal::NotWritable(t) | Refusal::Failed(t) => t,
        }
    }
}

impl From<String> for Refusal {
    fn from(text: String) -> Refusal {
        Refusal::Failed(text)
    }
}

pub trait Provider: Send + Sync {
    fn present<'a>(&'a self, fqdn: &'a Name, value: &'a str) -> Fut<'a>;

    fn cleanup<'a>(&'a self, fqdn: &'a Name, value: &'a str) -> Fut<'a>;
}

#[derive(Clone)]
pub struct Secret(Arc<Zeroizing<String>>);

impl Secret {
    pub fn new(value: String) -> Secret {
        Secret(Arc::new(Zeroizing::new(value)))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<secret>")
    }
}

impl PartialEq for Secret {
    fn eq(&self, other: &Secret) -> bool {
        self.expose() == other.expose()
    }
}

impl Eq for Secret {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsEntry {
    pub domains: Vec<String>,
    pub provider: DnsProvider,
    pub wait: Duration,
    pub resolvers: Vec<SocketAddr>,
    pub wildcards: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsProvider {
    Cloudflare {
        token: Secret,
    },
    Rfc2136 {
        server: String,
        key_name: String,
        secret: Secret,
    },
    AcmeDns {
        api: String,
        username: String,
        password: Secret,
        subdomain: String,
    },
    Command {
        path: PathBuf,
    },
}

impl DnsProvider {
    pub fn label(&self) -> &'static str {
        match self {
            DnsProvider::Cloudflare { .. } => "Cloudflare",
            DnsProvider::Rfc2136 { .. } => "RFC 2136",
            DnsProvider::AcmeDns { .. } => "acme-dns",
            DnsProvider::Command { .. } => "the command",
        }
    }
}

pub fn entry_for<'a>(entries: &'a [DnsEntry], name: &str) -> Option<(usize, &'a DnsEntry)> {
    let i = burrow_syntax::web::dns_entry_for(
        entries
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e.domains.as_slice())),
        name,
    )?;
    Some((i, entries.get(i)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(domains: &[&str], provider: DnsProvider) -> DnsEntry {
        DnsEntry {
            domains: domains.iter().map(|d| d.to_string()).collect(),
            provider,
            wait: DEFAULT_WAIT,
            resolvers: Vec::new(),
            wildcards: Vec::new(),
        }
    }

    #[test]
    fn secrets_never_print_and_compare_by_value() {
        let token = Secret::new("cf-token-123".into());
        let p = DnsProvider::Cloudflare {
            token: token.clone(),
        };
        let shown = format!("{p:?}");
        assert!(!shown.contains("cf-token-123"), "{shown}");
        assert!(shown.contains("<secret>"), "{shown}");
        assert_eq!(token, Secret::new("cf-token-123".into()));
        assert_ne!(token, Secret::new("cf-token-124".into()));
        assert_eq!(token.expose(), "cf-token-123");
    }

    #[test]
    fn a_name_uses_the_entry_of_its_most_specific_domain() {
        let list = vec![
            entry(
                &["example.com"],
                DnsProvider::Cloudflare {
                    token: Secret::new("t".into()),
                },
            ),
            entry(
                &["apps.example.com"],
                DnsProvider::Command {
                    path: "/usr/local/bin/burrow-dns".into(),
                },
            ),
        ];
        assert_eq!(entry_for(&list, "wiki.example.com").map(|e| e.0), Some(0));
        let (i, e) = entry_for(&list, "*.apps.example.com").unwrap();
        assert_eq!((i, e.provider.label()), (1, "the command"));
        assert!(entry_for(&list, "wiki.example.org").is_none());
    }
}
