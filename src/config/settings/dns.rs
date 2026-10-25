use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use zeroize::Zeroizing;

pub const DNS_WAIT: Duration = Duration::from_secs(120);

#[derive(Clone, PartialEq, Eq)]
pub struct SecretText(Zeroizing<String>);

impl SecretText {
    pub fn new(value: String) -> SecretText {
        SecretText(Zeroizing::new(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<secret>")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsSettings {
    pub domains: Vec<String>,
    pub provider: DnsProvider,
    pub wait: Duration,
    pub resolvers: Vec<SocketAddr>,
    pub wildcards: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsProvider {
    Cloudflare {
        token: SecretText,
    },
    Rfc2136 {
        server: String,
        key_name: String,
        secret: SecretText,
    },
    AcmeDns {
        api: String,
        username: SecretText,
        password: SecretText,
        subdomain: String,
    },
    Command {
        path: PathBuf,
    },
}
