use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WebSummary {
    pub https: String,
    #[serde(default)]
    pub addresses: String,
    #[serde(default)]
    pub shared_with_tunnel: bool,
    #[serde(default)]
    pub tunnel: Option<TunnelLine>,
    #[serde(default)]
    pub hosts: Vec<HostLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TunnelLine {
    pub url: String,
    #[serde(default)]
    pub certificate: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HostLine {
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub certificate: String,
    #[serde(default)]
    pub service: String,
}

impl WebSummary {
    pub fn head(&self) -> String {
        let shared = if self.shared_with_tunnel {
            "; shared with the tunnel"
        } else {
            ""
        };
        format!("web: https {} ({}{shared})", self.https, self.addresses)
    }

    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![self.head()];
        if let Some(t) = &self.tunnel {
            out.push(format!("tunnel: {} ({})", t.url, t.certificate));
        }
        for h in &self.hosts {
            out.push(h.line());
        }
        if self.hosts.is_empty() && self.tunnel.is_none() {
            out.push("no names yet: every connection is closed".into());
        }
        out
    }
}

impl HostLine {
    pub fn line(&self) -> String {
        format!(
            "{}: {}, {} -> service {}",
            self.name, self.kind, self.certificate, self.service
        )
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
