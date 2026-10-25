use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Set {
        path: Path,
        value: Value,
    },
    Remove {
        path: Path,
    },
    Add {
        path: Path,
        value: Value,
    },
    Rename {
        path: Path,
        to: String,
        #[serde(default)]
        refs: bool,
    },
    SecretAdd {
        path: Path,
        handle: String,
    },
    SecretRemove {
        path: Path,
        fp: String,
    },
    PasswordSet {
        path: Path,
        handle: String,
    },
    ReplaceDocument {
        from_history: String,
    },
    SetText {
        path: Path,
        text: String,
    },
}

impl Op {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Op::Set { path, .. }
            | Op::Remove { path }
            | Op::Add { path, .. }
            | Op::Rename { path, .. }
            | Op::SecretAdd { path, .. }
            | Op::SecretRemove { path, .. }
            | Op::PasswordSet { path, .. }
            | Op::SetText { path, .. } => Some(path),
            Op::ReplaceDocument { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ops_use_the_frozen_shape() {
        let ops: Vec<Op> = serde_json::from_value(json!([
            { "op": "set", "path": ["services","web","bind"], "value": "0.0.0.0:7840" },
            { "op": "remove", "path": ["services","web","allow_from"] },
            { "op": "add", "path": ["services","ssh"], "value": { "client": "home", "bind": 2222, "target": 22 } },
            { "op": "rename", "path": ["services","ssh"], "to": "ssh-home", "refs": true },
            { "op": "secret_add", "path": ["clients","home","secret"], "handle": "s1" },
            { "op": "secret_remove", "path": ["clients","home","secret"], "fp": "ab12cd34" },
            { "op": "replace_document", "from_history": "b2s:00" },
            { "op": "set_text", "path": ["future_section"], "text": "a: 1\nb: [x]" }
        ]))
        .unwrap();
        assert_eq!(ops.len(), 8);
        assert_eq!(ops[3].path().unwrap()[1], "ssh");
        let back = serde_json::to_value(&ops).unwrap();
        assert_eq!(back[4]["op"], "secret_add");
    }
}
