use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PLACEHOLDER_PATTERN: &str = "«secret [0-9a-f]{8}»";

const OPEN: &str = "«secret ";
const CLOSE: &str = "»";
const ID_LEN: usize = 8;

pub fn placeholder(id: &str) -> String {
    format!("{OPEN}{id}{CLOSE}")
}

pub fn placeholders_in(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(OPEN).map(|i| from + i) {
        let id_at = at + OPEN.len();
        let id = text.get(id_at..id_at + ID_LEN).unwrap_or_default();
        let hex = id.len() == ID_LEN && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if hex && text[id_at + ID_LEN..].starts_with(CLOSE) {
            let end = id_at + ID_LEN + CLOSE.len();
            out.push((at..end, id));
            from = end;
        } else {
            from = id_at;
        }
    }
    out
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
    fn placeholders_are_found_where_the_pattern_matches() {
        let text = "a: «secret 0123abcd» # «secret 0123ABCD» «secret 12» «secret ffffffff»";
        let found: Vec<&str> = placeholders_in(text)
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        assert_eq!(found, ["0123abcd", "ffffffff"]);
        let (range, _) = &placeholders_in(text)[0];
        assert_eq!(&text[range.clone()], placeholder("0123abcd"));
        assert!(placeholders_in("«secret «secret 00000000»").len() == 1);
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
