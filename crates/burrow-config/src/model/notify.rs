use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Notification")]
pub struct NotifyEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "POST each message as JSON to this URL, for example https://hooks.example.com/burrow"
    )]
    #[admin(
        help = "burrow POSTs each message as JSON to this address, for example https://hooks.example.com/burrow; http:// only to this machine or the LAN",
        none_label = "Not used"
    )]
    pub webhook: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Publish each message to this ntfy topic, for example https://ntfy.sh/my-burrow-topic"
    )]
    #[admin(
        label = "ntfy topic",
        help = "The full address of an ntfy topic, for example https://ntfy.sh/my-burrow-topic; pick a name nobody can guess",
        none_label = "Not used"
    )]
    pub ntfy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Send each message by email through an SMTP server")]
    #[admin(
        help = "Send each message by email through your mail server",
        none_label = "Not used"
    )]
    pub email: Option<EmailNotify>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Which events to send, for example [client_offline, client_back]; every event when not set"
    )]
    #[admin(
        ty = list(enum_(NotifyEvent)),
        help = "Which events this destination gets, for example only client_offline and client_back",
        empty_label = "All"
    )]
    pub events: Vec<NotifyEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Email")]
pub struct EmailNotify {
    #[schemars(description = "Who gets the messages, for example ops@example.com")]
    #[admin(help = "Who gets the messages, for example ops@example.com")]
    pub to: String,
    #[schemars(
        description = "The mail server: smtp://user@host:587 (STARTTLS) or smtps://user@host:465 (TLS); the password goes in password"
    )]
    #[admin(
        label = "Mail server",
        help = "smtp://user@mail.example.com:587 for STARTTLS or smtps://user@mail.example.com:465 for TLS; the password goes in its own field",
        locked
    )]
    pub smtp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The mail server password, from ${SMTP_PASS} or file:path; none when the server needs no login"
    )]
    #[admin(
        ty = secret_ref(max = 1, reference),
        help = "The mail account's password, from an environment variable like ${SMTP_PASS} or a file like file:/etc/burrow/smtp.pass",
        none_label = "No login",
        locked
    )]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The sender address, for example burrow@example.com; the user name of smtp when it is an address"
    )]
    #[admin(
        label = "Sender",
        help = "The sender address, for example burrow@example.com; most mail servers want the account's own address",
        none_label = "The mail server's user name"
    )]
    pub from: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, AdminSchema,
)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum NotifyEvent {
    #[admin(
        label = "Client offline",
        help = "A client lost its connection and did not come back within a minute"
    )]
    ClientOffline,
    #[admin(
        label = "Client back",
        help = "A client that was reported offline is connected again"
    )]
    ClientBack,
    #[admin(
        label = "Certificate failing",
        help = "A certificate cannot be got or renewed, and later when it works again"
    )]
    CertificateFailing,
    #[admin(
        label = "Service down",
        help = "A service's health check fails on every client"
    )]
    SiteDown,
    #[admin(
        label = "Service back",
        help = "A service reported down answers its health check again"
    )]
    SiteBack,
    #[admin(
        label = "Country database old",
        help = "The country database could not be updated for a long time, and later when it is fresh again"
    )]
    GeoipStale,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_event_reads_and_writes_its_wire_name() {
        for e in NotifyEvent::ALL.iter().copied() {
            let v = burrow_admin_proto::serde_json::to_value(e).unwrap();
            assert_eq!(v, burrow_admin_proto::serde_json::json!(e.wire()));
            assert_eq!(
                burrow_admin_proto::serde_json::from_value::<NotifyEvent>(v).unwrap(),
                e
            );
        }
    }

    #[test]
    fn an_entry_is_one_destination_with_its_events() {
        let e: NotifyEntry =
            burrow_admin_proto::serde_json::from_value(burrow_admin_proto::serde_json::json!({
                "ntfy": "https://ntfy.sh/t",
                "events": ["site_down", "site_back"]
            }))
            .unwrap();
        assert_eq!(e.ntfy.as_deref(), Some("https://ntfy.sh/t"));
        assert_eq!(e.events, [NotifyEvent::SiteDown, NotifyEvent::SiteBack]);
        assert!(
            burrow_admin_proto::serde_json::from_value::<NotifyEntry>(
                burrow_admin_proto::serde_json::json!({"slack": "x"})
            )
            .is_err()
        );
    }
}
