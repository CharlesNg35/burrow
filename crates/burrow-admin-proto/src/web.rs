use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WebSummary {
    #[serde(default)]
    pub https: String,
    #[serde(default)]
    pub addresses: String,
    #[serde(default)]
    pub http: String,
    #[serde(default)]
    pub http_addresses: String,
    #[serde(default)]
    pub shared_with_tunnel: bool,
    #[serde(default)]
    pub http_shared_with_tunnel: bool,
    #[serde(default)]
    pub certificates: String,
    #[serde(default)]
    pub tunnel: Option<TunnelLine>,
    #[serde(default)]
    pub hosts: Vec<HostLine>,
    #[serde(default)]
    pub http_note: String,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub front: String,
    #[serde(default)]
    pub wildcards: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TunnelLine {
    pub url: String,
    #[serde(default)]
    pub certificate: String,
}

pub const PLAIN: &str = "plain HTTP";

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HostLine {
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub certificate: String,
    #[serde(default)]
    pub check: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub routes: Vec<RouteLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RouteLine {
    pub path: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service: String,
}

pub const NOT_FOUND: &str = "everything else";

impl WebSummary {
    pub fn head(&self) -> String {
        let shared = if self.shared_with_tunnel {
            "; shared with the tunnel"
        } else {
            ""
        };
        let mut ports = Vec::new();
        if !self.https.is_empty() {
            ports.push(format!("https {} ({}{shared})", self.https, self.addresses));
        }
        if !self.http.is_empty() {
            let shared = if self.http_shared_with_tunnel {
                "; shared with the tunnel"
            } else {
                ""
            };
            ports.push(format!(
                "http {} ({}{shared})",
                self.http, self.http_addresses
            ));
        }
        format!("web: {}", ports.join(", "))
    }

    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![self.head()];
        if !self.front.is_empty() {
            out.push(self.front.clone());
        }
        if !self.certificates.is_empty() {
            out.push(format!("certificates: {}", self.certificates));
        }
        if !self.account.is_empty() {
            out.push(format!("account: {}", self.account));
        }
        if let Some(t) = &self.tunnel {
            out.push(format!("tunnel: {} ({})", t.url, t.certificate));
        }
        out.extend(self.wildcards.iter().cloned());
        for h in &self.hosts {
            out.push(h.line());
            out.extend(
                h.routes
                    .iter()
                    .map(|r| format!("  {:<11} -> {}", r.path, r.to)),
            );
        }
        if self.hosts.is_empty() && self.tunnel.is_none() {
            out.push("no names yet: every connection is closed".into());
        }
        if !self.http.is_empty() && !self.http_note.is_empty() {
            out.push(format!("http {}: {}", self.http, self.http_note));
        }
        out
    }
}

impl HostLine {
    pub fn served(&self) -> String {
        let mut out = self.certificate.clone();
        if !self.check.is_empty() {
            out.push_str(", ");
            out.push_str(&self.check);
        }
        if !self.note.is_empty() {
            out.push_str("; ");
            out.push_str(&self.note);
        }
        out
    }

    pub fn line(&self) -> String {
        if !self.routes.is_empty() {
            return format!("{}: {}", self.name, self.served());
        }
        format!(
            "{}: {} -> service {}",
            self.name,
            self.served(),
            self.service
        )
    }

    pub fn first_service(&self) -> Option<&str> {
        std::iter::once(self.service.as_str())
            .chain(self.routes.iter().map(|r| r.service.as_str()))
            .find(|s| !s.is_empty())
    }

    pub fn unencrypted(&self) -> bool {
        self.kind == PLAIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_status_without_web_fields_still_reads() {
        let s: WebSummary =
            serde_json::from_value(serde_json::json!({"https": "[::]:443"})).unwrap();
        assert_eq!(
            s.lines(),
            [
                "web: https [::]:443 ()",
                "no names yet: every connection is closed"
            ]
        );
        let status: crate::status::Status = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(status.web, None);
        assert!(serde_json::to_value(&status).unwrap().get("web").is_none());
    }
}
