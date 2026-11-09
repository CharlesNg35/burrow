use std::collections::{BTreeMap, BTreeSet};

use burrow_admin_proto::text_edit::EditText;
use leptos::prelude::*;

use super::TextEdit;

pub fn gone_edits(
    old: Option<&EditText>,
    next: &EditText,
    values: &BTreeMap<String, String>,
    unsaved: &BTreeSet<String>,
    new: &[String],
) -> Vec<String> {
    let exists = |n: &str| next.files.iter().any(|f| f.name == n) || new.iter().any(|x| x == n);
    let mut out: Vec<String> = old
        .map(|o| super::text::changed_of(&o.files, values))
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.name)
        .chain(unsaved.iter().cloned())
        .filter(|n| !exists(n))
        .collect();
    out.sort();
    out.dedup();
    out
}

pub fn gone_note(gone: &[String]) -> Option<String> {
    match gone {
        [] => None,
        [one] => Some(format!(
            "{one} was removed on the server; its edits here are gone"
        )),
        [one, rest @ ..] => Some(format!(
            "{one} and {} were removed on the server; their edits here are gone",
            burrow_admin_logic::wording::count_of(rest.len(), "other file")
        )),
    }
}

impl TextEdit {
    pub(super) fn prune(&self, next: &EditText, new: &[String]) {
        let exists = |n: &String| next.files.iter().any(|f| f.name == *n) || new.contains(n);
        if self
            .unsaved
            .with_untracked(|u| u.iter().any(|n| !exists(n)))
        {
            self.unsaved.update(|u| u.retain(exists));
        }
        for map in [self.edit_as, self.shown] {
            if map.with_untracked(|m| m.keys().any(|n| !exists(n))) {
                map.update(|m| m.retain(|n, _| exists(n)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::text_edit::EditFile;

    fn text(files: &[(&str, &str)]) -> EditText {
        EditText {
            files: files
                .iter()
                .map(|(n, t)| EditFile {
                    name: n.to_string(),
                    text: t.to_string(),
                    read_only: None,
                })
                .collect(),
            ..EditText::default()
        }
    }

    #[test]
    fn edits_in_a_file_the_server_removed_are_named_once() {
        let old = text(&[
            ("server.yaml", "a: 1\n"),
            ("server.d/x.yaml", "b: 1\n"),
            ("server.d/y.yaml", "c: 1\n"),
        ]);
        let next = text(&[("server.yaml", "a: 1\n")]);
        let values: BTreeMap<String, String> = [
            ("server.yaml", "a: 1\n"),
            ("server.d/x.yaml", "b: 2\n"),
            ("server.d/y.yaml", "c: 1\n"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let gone = gone_edits(Some(&old), &next, &values, &BTreeSet::new(), &[]);
        assert_eq!(
            gone,
            ["server.d/x.yaml"],
            "an untouched removed file is not named"
        );
        assert_eq!(
            gone_note(&gone).as_deref(),
            Some("server.d/x.yaml was removed on the server; its edits here are gone")
        );
        let unsaved: BTreeSet<String> = ["server.d/y.yaml".to_string()].into();
        let both = gone_edits(Some(&old), &next, &values, &unsaved, &[]);
        assert_eq!(both, ["server.d/x.yaml", "server.d/y.yaml"]);
        assert!(
            gone_note(&both)
                .unwrap()
                .contains("and 1 other file were removed")
        );
        assert_eq!(gone_note(&[]), None);
    }

    #[test]
    fn a_removed_file_leaves_no_state_behind() {
        let t = TextEdit::new();
        t.unsaved.update(|u| {
            u.insert("server.d/x.yaml".into());
            u.insert("server.yaml".into());
        });
        t.edit_as.update(|m| {
            m.insert("server.d/x.yaml".into(), "TOML".into());
        });
        t.shown.update(|m| {
            m.insert("server.d/x.yaml".into(), "TOML".into());
        });
        t.prune(&text(&[("server.yaml", "a: 1\n")]), &[]);
        assert_eq!(
            t.unsaved.get_untracked(),
            ["server.yaml".to_string()].into()
        );
        assert!(t.edit_as.get_untracked().is_empty() && t.shown.get_untracked().is_empty());
    }
}
