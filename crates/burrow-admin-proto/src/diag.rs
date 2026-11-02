use serde::{Deserialize, Serialize};

use crate::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl<'de> Deserialize<'de> for Severity {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "error" => Severity::Error,
            "info" => Severity::Info,
            _ => Severity::Warning,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diag {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub path: Option<Path>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also: Vec<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub before: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subjects: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

impl Diag {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    pub fn concerns(&self, path: &[String]) -> bool {
        self.path.as_deref() == Some(path) || self.also.iter().any(|p| p == path)
    }

    pub fn within(&self, prefix: &[String]) -> bool {
        self.path.as_ref().is_some_and(|p| p.starts_with(prefix))
            || self.also.iter().any(|p| p.starts_with(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_severity_is_a_warning() {
        let d: Diag = serde_json::from_value(json!({
            "severity": "notice", "code": "x", "message": "m", "path": ["a"]
        }))
        .unwrap();
        assert_eq!(d.severity, Severity::Warning);
        assert!(d.concerns(&["a".to_string()]));
        assert!(!d.is_error());
    }
}
