use serde::{Deserialize, Serialize};

pub const RUN_PREFIX: &str = "/r-";
pub const TOKEN_FRAGMENT: &str = "#token=";
pub const API_PATH: &str = "/api";
pub const BRIDGE_EVENT: &str = "bridge";
pub const UI_CLIENT: &str = "burrow-admin-ui";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Auth {
    pub auth: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BridgeState {
    Open,
    Refused,
    #[serde(rename = "server-closed")]
    ServerClosed,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    Forbidden,
    Busy,
    NoAdmin,
    Refused,
    Rejected,
    Unreachable,
    #[serde(other)]
    Other,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Forbidden => "forbidden",
            Reason::Busy => "busy",
            Reason::NoAdmin => "no-admin",
            Reason::Refused => "refused",
            Reason::Rejected => "rejected",
            Reason::Unreachable => "unreachable",
            Reason::Other => "other",
        }
    }

    pub fn passes(self) -> bool {
        matches!(self, Reason::Unreachable | Reason::Busy)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bridge {
    pub state: BridgeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub server: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bridge_event_round_trips_and_an_unknown_reason_is_other() {
        let b = Bridge {
            state: BridgeState::Refused,
            reason: Some(Reason::NoAdmin),
            message: "m".into(),
            server: String::new(),
        };
        let text = serde_json::to_string(&b).unwrap();
        assert!(text.contains("\"no-admin\""), "{text}");
        assert_eq!(serde_json::from_str::<Bridge>(&text).unwrap(), b);
        let later: Bridge =
            serde_json::from_str(r#"{"state":"paused","reason":"brand-new"}"#).unwrap();
        assert_eq!(later.state, BridgeState::Unknown);
        assert_eq!(later.reason, Some(Reason::Other));
    }
}
