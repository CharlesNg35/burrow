use serde::{Deserialize, Serialize};

use crate::Path;
use crate::diag::Diag;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Plan {
    pub plan: String,
    pub base: String,
    pub result_rev: String,
    pub valid: bool,
    #[serde(default)]
    pub diags: Vec<Diag>,
    #[serde(default)]
    pub changes: Vec<Change>,
    #[serde(default)]
    pub diff: Diff,
    #[serde(default)]
    pub impacts: Vec<Impact>,
    #[serde(default)]
    pub refusals: Vec<Refusal>,
    #[serde(default)]
    pub applies: Vec<ApplyHow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reformat: Option<Reformat>,
    #[serde(default)]
    pub secrets_in_diff: String,
}

impl Plan {
    pub fn errors(&self) -> usize {
        self.diags.iter().filter(|d| d.is_error()).count()
    }

    pub fn confirms(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self
            .impacts
            .iter()
            .filter_map(|i| i.confirm.as_deref())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn can_apply(&self) -> bool {
        self.valid && self.refusals.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Add,
    Remove,
    Set,
    Rename,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub kind: ChangeKind,
    pub path: Path,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_of: Option<Path>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Diff {
    #[serde(default)]
    pub unified: String,
    #[serde(default)]
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Danger,
    Warning,
    #[serde(other)]
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Impact {
    pub level: Level,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyHow {
    pub field: String,
    pub how: crate::schema::Applies,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reformat {
    pub scope: Path,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Applied {
    pub rev: String,
    #[serde(default)]
    pub applied: Vec<ApplyHow>,
    #[serde(default)]
    pub restart_needed: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<RevealedSecret>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snippets: Vec<Snippets>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevealedSecret {
    pub handle: String,
    pub path: Path,
    pub fp: String,
    pub value: String,
    #[serde(default)]
    pub shown_once: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Snippets {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub server_known: bool,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub config: String,
    #[serde(default)]
    pub with_secret: bool,
    #[serde(default)]
    pub secret_fp: Option<String>,
    #[serde(default)]
    pub env_var: String,
    #[serde(default)]
    pub env_config: String,
    #[serde(default)]
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub os: String,
    pub label: String,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Conflict {
    pub current: String,
    #[serde(default)]
    pub theirs: Vec<Change>,
    #[serde(default)]
    pub known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReloadFailed {
    pub restored: bool,
    pub reason: String,
    #[serde(default)]
    pub hint: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plans_decode_with_defaults() {
        let p: Plan = serde_json::from_value(json!({
            "plan": "b2s:1", "base": "b2s:0", "result_rev": "b2s:2", "valid": true,
            "impacts": [
                {"level": "danger", "code": "opens-public-port", "message": "m", "confirm": "ssh"},
                {"level": "catastrophe", "code": "x", "message": "m"}
            ],
            "diff": {"unified": "", "hunks": [{"old_start": 1, "old_lines": 1, "new_start": 1, "new_lines": 2,
                     "lines": [[" ", "a"], ["+", "b"]]}]}
        }))
        .unwrap();
        assert_eq!(p.impacts[1].level, Level::Info);
        assert_eq!(p.confirms(), vec!["ssh"]);
        assert!(p.can_apply());
        assert_eq!(p.diff.hunks[0].lines[1].0, "+");
    }
}
