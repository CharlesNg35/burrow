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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_ca: Option<LocalCaStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_waiting: Option<PortWait>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geoip: Option<GeoipStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notify: Vec<NotifyStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GeoipStatus {
    pub source: String,
    #[serde(default)]
    pub file: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub credit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<u64>,
    #[serde(default)]
    pub result: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

impl GeoipStatus {
    pub fn ready(&self) -> bool {
        self.built.is_some()
    }

    pub fn line(&self, now: u64) -> String {
        let held = match self.built {
            Some(b) => format!(
                "{} from {}, {} days old",
                self.source,
                crate::date::year_month(b),
                now.saturating_sub(b) / 86_400
            ),
            None => format!("{}: no database yet", self.source),
        };
        let checked = self
            .checked
            .map(|c| format!("; last check {}", crate::date::date_time(c)))
            .unwrap_or_default();
        let result = if self.result.is_empty() {
            String::new()
        } else {
            format!(": {}", self.result)
        };
        format!("{held}{checked}{result}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NotifyStatus {
    pub name: String,
    #[serde(default)]
    pub sent: u64,
    #[serde(default)]
    pub waiting: u64,
    #[serde(default)]
    pub dropped: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failing_secs: Option<u64>,
}

impl NotifyStatus {
    pub fn line(&self) -> Option<String> {
        let why = self.failing.as_deref()?;
        Some(format!(
            "Notifications to {} fail: {why}; burrow keeps trying",
            self.name
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PortWait {
    pub key: String,
    pub addr: String,
    pub reason: String,
    #[serde(default)]
    pub stays: String,
    #[serde(default)]
    pub retry_secs: u64,
}

impl PortWait {
    pub fn line(&self) -> String {
        let stays = if self.stays.is_empty() {
            "stays off".to_string()
        } else {
            format!("stays on {}", self.stays)
        };
        format!(
            "{} {} is {}; the web proxy {stays} and tries again every {}s",
            self.key, self.addr, self.reason, self.retry_secs
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LocalCaStatus {
    pub subject: String,
    pub sha256: String,
    pub not_before: u64,
    pub not_after: u64,
    pub root_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
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
    #[serde(default, skip_serializing_if = "is_zero")]
    pub refused_country: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_countries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_countries: Vec<String>,
    #[serde(default)]
    pub bytes_in: u64,
    #[serde(default)]
    pub bytes_out: u64,
    #[serde(default)]
    pub health: Vec<HealthStatus>,
    #[serde(default)]
    pub set_aside: Vec<SetAside>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<EntryStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub web: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpCounts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HttpCounts {
    pub responses: [u64; 5],
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SetAside {
    pub client: String,
    #[serde(default)]
    pub refused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EntryStatus {
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default)]
    pub open: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HealthStatus {
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default)]
    pub up: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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
    LocalCa,
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

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_web_port_wait_says_what_waits_and_where_the_proxy_stays() {
        let mut w = PortWait {
            key: "web.https".into(),
            addr: "0.0.0.0:8443".into(),
            reason: "in use".into(),
            stays: "https 0.0.0.0:443".into(),
            retry_secs: 5,
        };
        assert_eq!(
            w.line(),
            "web.https 0.0.0.0:8443 is in use; the web proxy stays on https 0.0.0.0:443 and tries again every 5s"
        );
        w.stays.clear();
        assert_eq!(
            w.line(),
            "web.https 0.0.0.0:8443 is in use; the web proxy stays off and tries again every 5s"
        );
    }

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
