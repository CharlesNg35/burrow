use std::rc::Rc;

use leptos::prelude::*;

use super::TextEdit;
pub use super::unwritten::Unwritten;

pub type Ask = Rc<dyn Fn(&str, &str)>;

pub fn own_label(name: &str) -> &'static str {
    if name.to_ascii_lowercase().ends_with(".json") {
        "JSON"
    } else {
        "YAML"
    }
}

impl TextEdit {
    pub fn shown_as(&self, name: &str, label: &str) {
        if self
            .shown
            .with_untracked(|s| s.get(name).map(String::as_str) != Some(label))
        {
            self.shown.update(|s| {
                s.insert(name.to_string(), label.to_string());
            });
        }
        if self
            .edit_as
            .with_untracked(|a| a.get(name).map(String::as_str) != Some(label))
        {
            self.edit_as.update(|a| {
                a.insert(name.to_string(), label.to_string());
            });
        }
        if let Some(u) = self.held.with_untracked(|h| h.get(name).cloned())
            && u.shown != label
        {
            self.held.update(|h| {
                h.insert(
                    name.to_string(),
                    Unwritten {
                        shown: label.to_string(),
                        ..u
                    },
                );
            });
        }
    }

    pub fn synced(&self, name: &str, in_sync: bool, text: Option<String>) {
        if in_sync {
            if self.held.with_untracked(|h| h.contains_key(name)) {
                self.held.update(|h| {
                    h.remove(name);
                });
            }
            return;
        }
        let own = own_label(name);
        let shown = self
            .shown
            .with_untracked(|s| s.get(name).cloned())
            .filter(|s| s != own)
            .or_else(|| self.edit_as.with_untracked(|a| a.get(name).cloned()))
            .unwrap_or_else(|| own.to_string());
        let text = text
            .or_else(|| {
                self.held
                    .with_untracked(|h| h.get(name).map(|u| u.text.clone()))
            })
            .unwrap_or_default();
        self.held.update(|h| {
            h.insert(
                name.to_string(),
                Unwritten {
                    name: name.to_string(),
                    shown,
                    own: own.to_string(),
                    text,
                },
            );
        });
    }

    pub fn keep_text(&self, name: &str, text: String) {
        if self
            .held
            .with_untracked(|h| h.get(name).is_some_and(|u| u.text != text))
        {
            self.held.update(|h| {
                if let Some(u) = h.get_mut(name) {
                    u.text = text;
                }
            });
        }
    }

    pub fn capture(&self) {
        let _ = self
            .before_review
            .try_with_value(|h| h.as_ref().map(|h| h()));
    }

    pub fn held_now(&self) -> Vec<Unwritten> {
        self.capture();
        self.held.with_untracked(|h| h.values().cloned().collect())
    }

    pub fn editor_errors(&self, name: &str, first: Option<String>) {
        let changed = self
            .errors
            .with_untracked(|e| e.get(name) != first.as_ref());
        if changed {
            self.errors.update(|e| match first {
                Some(why) => {
                    e.insert(name.to_string(), why);
                }
                None => {
                    e.remove(name);
                }
            });
        }
        let why = self.errors.with_untracked(|e| e.values().next().cloned());
        if self.unsavable.with_untracked(|u| *u != why) {
            self.unsavable.set(why);
        }
    }

    pub fn ask_again(&self, name: &str, label: &str) {
        let _ = self
            .ask
            .try_with_value(|a| a.as_ref().map(|a| a(name, label)));
    }

    pub(super) fn forget(&self, name: &str) {
        self.unsaved.update(|s| {
            s.remove(name);
        });
        if self.held.with_untracked(|h| h.contains_key(name)) {
            self.held.update(|h| {
                h.remove(name);
            });
        }
        self.editor_errors(name, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_on_screen_is_what_the_editor_reports() {
        let t = TextEdit::new();
        t.edit_as.update(|a| {
            a.insert("server.yaml".into(), "TOML".into());
        });
        t.shown_as("server.yaml", "TOML");
        assert_eq!(t.shown.get_untracked()["server.yaml"], "TOML");
        t.shown_as("server.yaml", "YAML");
        assert_eq!(
            t.edit_as.get_untracked()["server.yaml"],
            "YAML",
            "an undo back across the switch: Edit as follows the screen, nothing waits"
        );
    }

    #[test]
    fn a_new_server_value_the_edit_language_cannot_hold_shows_in_the_files_language() {
        let t = TextEdit::new();
        t.edit_as.update(|a| {
            a.insert("server.yaml".into(), "TOML".into());
        });
        t.shown_as("server.yaml", "TOML");
        t.shown_as("server.yaml", "YAML");
        let (shown, asked) = (
            t.shown.get_untracked()["server.yaml"].clone(),
            t.edit_as.get_untracked()["server.yaml"].clone(),
        );
        assert_eq!(
            (shown.as_str(), asked.as_str()),
            ("YAML", "YAML"),
            "the select follows the editor to the file's language and nothing claims to wait"
        );
    }

    #[test]
    fn text_out_of_sync_is_held_until_the_editor_says_it_is_written_back() {
        let t = TextEdit::new();
        t.shown_as("server.yaml", "TOML");
        t.synced("server.yaml", false, Some("broken = [\n".into()));
        let held = t.held.get_untracked()["server.yaml"].clone();
        assert_eq!(
            (held.shown.as_str(), held.own.as_str(), held.text.as_str()),
            ("TOML", "YAML", "broken = [\n")
        );
        t.keep_text("server.yaml", "broken = [1\n".into());
        assert_eq!(t.held.get_untracked()["server.yaml"].text, "broken = [1\n");
        t.synced("server.yaml", true, None);
        assert!(t.held.get_untracked().is_empty());
    }

    #[test]
    fn the_editors_own_errors_hold_the_review() {
        let t = TextEdit::new();
        t.editor_errors(
            "server.d/a.yaml",
            Some("server.d/a.yaml line 2: bad".into()),
        );
        assert_eq!(
            t.unsavable.get_untracked().as_deref(),
            Some("server.d/a.yaml line 2: bad")
        );
        t.forget("server.d/a.yaml");
        assert!(
            t.unsavable.get_untracked().is_none(),
            "a discarded file holds nothing"
        );
    }

    #[test]
    fn an_edit_only_the_editor_holds_counts_as_a_change() {
        use burrow_admin_proto::text_edit::{EditFile, EditText};
        let t = TextEdit::new();
        t.loaded.set(Some(Ok(std::sync::Arc::new(EditText {
            files: vec![EditFile {
                name: "server.yaml".into(),
                text: "a: 1\n".into(),
                read_only: None,
            }],
            ..EditText::default()
        }))));
        assert!(!t.dirty());
        t.set_unsaved("server.yaml", true);
        assert!(
            t.dirty(),
            "Cancel asks, leaving the page warns, the form waits"
        );
    }
}
