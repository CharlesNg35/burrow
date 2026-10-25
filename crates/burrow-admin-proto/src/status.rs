use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub uptime_secs: u64,
    #[serde(default)]
    pub listening: Vec<String>,
    #[serde(default)]
    pub handshakes_refused: u64,
    #[serde(default)]
    pub udp_peers: u64,
    #[serde(default)]
    pub clients: Vec<ClientStatus>,
    #[serde(default)]
    pub services: Vec<ServiceStatus>,
    #[serde(default)]
    pub visitors: Vec<VisitorStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<crate::web::WebSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub certificates: Vec<CertStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acme: Option<AcmeStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AcmeStatus {
    pub directory: String,
    #[serde(default)]
    pub account: Option<String>,
    pub state_dir: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queue: Vec<CertOrderStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CertOrderStatus {
    pub service: String,
    pub names: Vec<String>,
    pub at: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ClientStatus {
    pub name: String,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub connected_secs: Option<u64>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub open: u64,
    #[serde(default)]
    pub idle_channels: u64,
    #[serde(default)]
    pub left_secs_ago: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
    #[serde(default)]
    pub busy_total: u64,
    #[serde(default)]
    pub at_limit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: String,
    #[serde(default)]
    pub protocol: String,
    #[serde(default)]
    pub bind: Option<String>,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub clients: Vec<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub bandwidth: Option<u64>,
    #[serde(default)]
    pub visitors: Vec<String>,
    #[serde(default)]
    pub proxy_protocol: Option<String>,
    #[serde(default)]
    pub filtered: bool,
    #[serde(default)]
    pub open: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub refused: u64,
    #[serde(default)]
    pub bytes_in: u64,
    #[serde(default)]
    pub bytes_out: u64,
    #[serde(default)]
    pub health: Vec<HealthStatus>,
    #[serde(default)]
    pub set_aside: Vec<SetAside>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub web: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SetAside {
    pub client: String,
    #[serde(default)]
    pub refused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HealthStatus {
    pub client: String,
    #[serde(default)]
    pub up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VisitorStatus {
    pub name: String,
    #[serde(default)]
    pub open: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub last_seen_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertStatus {
    pub name: String,
    pub source: CertSourceStatus,
    #[serde(default)]
    pub check: Option<CertCheck>,
    pub state: CertStateStatus,
    #[serde(default)]
    pub expires: Option<u64>,
    #[serde(default)]
    pub next_renewal: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    #[serde(default)]
    pub failures: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CertSourceStatus {
    OwnFile { path: String },
    SharedFile { path: String },
    TunnelFile { path: String },
    LetsEncrypt { directory: String },
    Passthrough,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertCheck {
    Tls,
    Http,
    Dns,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CertStateStatus {
    Ok,
    Pending,
    Renewing,
    Failing { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_certificate_line_reads_back_and_is_left_out_while_there_are_none() {
        let c = CertStatus {
            name: "nas.example.com".into(),
            source: CertSourceStatus::LetsEncrypt {
                directory: "https://acme-v02.api.letsencrypt.org/directory".into(),
            },
            check: Some(CertCheck::Tls),
            state: CertStateStatus::Failing {
                reason: "rate limited".into(),
            },
            expires: Some(1_800_000_000),
            next_renewal: None,
            last_error: Some("rate limited".into()),
            code: None,
            explanation: Some("https://example.com/why".into()),
            failures: 3,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["source"]["kind"], "lets_encrypt");
        assert_eq!(
            v["state"],
            serde_json::json!({"kind": "failing", "reason": "rate limited"})
        );
        assert_eq!(serde_json::from_value::<CertStatus>(v.clone()).unwrap(), c);
        assert_eq!(v["failures"], 3);
        assert_eq!(v["explanation"], "https://example.com/why");
        let empty = serde_json::to_value(Status::default()).unwrap();
        assert!(empty.get("certificates").is_none() && empty.get("acme").is_none());
        let acme = AcmeStatus {
            directory: "https://acme-v02.api.letsencrypt.org/directory".into(),
            account: Some("https://acme-v02.api.letsencrypt.org/acme/acct/1".into()),
            state_dir: "/var/lib/burrow/home".into(),
            queue: vec![CertOrderStatus {
                service: "blog".into(),
                names: vec!["blog.example.com".into()],
                at: 1_800_000_000,
                running: true,
            }],
        };
        let v = serde_json::to_value(&acme).unwrap();
        assert_eq!(v["state_dir"], "/var/lib/burrow/home");
        assert_eq!(v["queue"][0]["names"][0], "blog.example.com");
        assert_eq!(v["queue"][0]["running"], true);
        assert_eq!(serde_json::from_value::<AcmeStatus>(v).unwrap(), acme);
    }
}
