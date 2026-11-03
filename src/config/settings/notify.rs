use std::collections::BTreeSet;

use super::SecretText;
use burrow_config::NotifyEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyTarget {
    pub via: NotifyVia,
    pub events: BTreeSet<NotifyEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyVia {
    Webhook(String),
    Ntfy(String),
    Email(EmailSettings),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailSettings {
    pub to: String,
    pub from: String,
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub user: Option<String>,
    pub password: Option<SecretText>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpSecurity {
    Tls,
    StartTls,
    StartTlsIfOffered,
}

impl NotifyTarget {
    pub fn wants(&self, event: NotifyEvent) -> bool {
        self.events.is_empty() || self.events.contains(&event)
    }

    pub fn shown(&self) -> String {
        match &self.via {
            NotifyVia::Webhook(url) => format!("webhook {}", host_of(url)),
            NotifyVia::Ntfy(url) => format!("ntfy {}", host_of(url)),
            NotifyVia::Email(e) => format!("email {}", e.to),
        }
    }
}

fn host_of(url: &str) -> String {
    url.parse::<http::Uri>()
        .ok()
        .and_then(|u| u.host().map(str::to_string))
        .unwrap_or_else(|| "(invalid address)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_destination_is_shown_without_its_path_where_webhook_tokens_live() {
        let t = |via| NotifyTarget {
            via,
            events: BTreeSet::new(),
        };
        let hook = t(NotifyVia::Webhook(
            "https://hooks.slack.com/services/T0/B0/XXXX".into(),
        ));
        assert_eq!(hook.shown(), "webhook hooks.slack.com");
        assert_eq!(
            t(NotifyVia::Ntfy("https://ntfy.sh/secret-topic".into())).shown(),
            "ntfy ntfy.sh"
        );
        assert!(hook.wants(NotifyEvent::GeoipStale));
    }
}
