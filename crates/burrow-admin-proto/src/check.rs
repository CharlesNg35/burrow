use serde::{Deserialize, Serialize};

use crate::codes;

pub const MAX_TEXT: usize = 64 * 1024;
pub const WHOLE_FILE_ONLY: [&str; 3] = [codes::NO_LISTENER, codes::NO_CLIENTS, codes::NO_SERVICES];
pub const READ_WHERE_BURROW_RUNS: [&str; 2] = [codes::ENV_UNSET, codes::TLS_UNREADABLE];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum CheckRole {
    #[default]
    Server,
    Client,
    Visitor,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum CheckScope {
    #[default]
    Fragment,
    Whole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckText {
    pub text: String,
    #[serde(default)]
    pub role: CheckRole,
    #[serde(default)]
    pub scope: CheckScope,
}

pub fn too_large(len: usize) -> Option<String> {
    (len > MAX_TEXT)
        .then(|| format!("the config is {len} bytes; at most {MAX_TEXT} can be checked at once"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_request_defaults_to_a_server_fragment_and_has_a_size_cap() {
        let r: CheckText = serde_json::from_value(serde_json::json!({ "text": "x: 1" })).unwrap();
        assert_eq!((r.role, r.scope), (CheckRole::Server, CheckScope::Fragment));
        assert!(too_large(MAX_TEXT).is_none());
        assert!(too_large(MAX_TEXT + 1).unwrap().contains("at most 65536"));
    }
}
