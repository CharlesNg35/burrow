use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PLACEHOLDER_PATTERN: &str = "«secret [0-9a-f]{8}»";

pub fn placeholder(id: &str) -> String {
    format!("«secret {id}»")
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EditText {
    pub rev: String,
    pub schema_hash: String,
    pub schema: Value,
    pub placeholder: String,
    #[serde(default)]
    pub files: Vec<EditFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EditFile {
    pub name: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct TextFile {
    pub name: String,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placeholder_matches_its_own_pattern_shape() {
        let p = placeholder("3f2a9c01");
        assert_eq!(p, "«secret 3f2a9c01»");
        let id = p.strip_prefix("«secret ").and_then(|r| r.strip_suffix('»'));
        assert!(id.is_some_and(|id| id.len() == 8 && id.bytes().all(|b| b.is_ascii_hexdigit())));
        assert!(PLACEHOLDER_PATTERN.contains("[0-9a-f]{8}"));
    }

    #[test]
    fn a_read_only_file_says_why_and_a_writable_one_says_nothing() {
        let f = EditFile {
            name: "server.d/10-web.yaml".into(),
            text: "services: {}\n".into(),
            read_only: Some("not writable by burrow".into()),
        };
        let back: EditFile = serde_json::from_value(serde_json::to_value(&f).unwrap()).unwrap();
        assert_eq!(back, f);
        let plain = serde_json::to_value(EditFile::default()).unwrap();
        assert!(plain.get("read_only").is_none());
    }
}
