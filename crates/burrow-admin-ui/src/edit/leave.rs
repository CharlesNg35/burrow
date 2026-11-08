use std::rc::Rc;

use leptos::prelude::*;

use super::TextEdit;

#[derive(Clone, Debug, PartialEq)]
pub struct Unwritten {
    pub name: String,
    pub shown: String,
    pub own: String,
    pub text: String,
}

impl Unwritten {
    pub fn reason(&self) -> String {
        format!(
            "{} cannot be written as {} yet; fix the text first",
            self.name, self.own
        )
    }

    pub fn title(&self) -> String {
        format!("Leave and lose the {} text?", self.shown)
    }

    pub fn question(&self) -> String {
        format!(
            "{} has {} text that cannot be saved as {} yet.",
            self.name, self.shown, self.own
        )
    }
}

pub type Then = Rc<dyn Fn()>;

impl TextEdit {
    pub fn unwritten(&self) -> Option<Unwritten> {
        if !self.open.get_untracked() {
            return None;
        }
        let u = self
            .before_review
            .try_with_value(|h| h.as_ref().and_then(|h| h()))
            .flatten()?;
        self.stuck.update(|s| {
            s.insert(u.name.clone(), (u.shown.clone(), u.text.clone()));
        });
        Some(u)
    }

    pub fn may_leave(&self, then: impl Fn() + 'static) -> bool {
        if self.let_through.get_value() {
            self.let_through.set_value(false);
            return true;
        }
        let Some(u) = self.unwritten() else {
            return true;
        };
        self.then.set_value(Some(Rc::new(then)));
        self.leaving.set(Some(u));
        false
    }

    pub fn stay(&self) {
        self.then.set_value(None);
        if self.leaving.with_untracked(Option::is_some) {
            self.leaving.set(None);
        }
    }

    pub fn leave(&self) {
        let Some(u) = self.leaving.get_untracked() else {
            return;
        };
        self.stuck.update(|s| {
            s.remove(&u.name);
        });
        self.unsaved.update(|s| {
            s.remove(&u.name);
        });
        self.leaving.set(None);
        let then = self.then.try_update_value(Option::take).flatten();
        self.let_through.set_value(true);
        if let Some(then) = then {
            then();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stuck() -> Unwritten {
        Unwritten {
            name: "server.yaml".into(),
            shown: "TOML".into(),
            own: "YAML".into(),
            text: "heartbeat =\n".into(),
        }
    }

    #[test]
    fn leaving_asks_only_while_a_file_holds_text_that_cannot_be_written_back() {
        let t = TextEdit::new();
        t.open.set(true);
        let gone = RwSignal::new(0);
        assert!(
            t.may_leave(move || gone.update(|n| *n += 1)),
            "nothing held: leave at once"
        );
        t.before_review.set_value(Some(Rc::new(|| Some(stuck()))));
        assert!(!t.may_leave(move || gone.update(|n| *n += 1)));
        assert_eq!(t.leaving.get_untracked(), Some(stuck()));
        assert_eq!(
            t.stuck.get_untracked()["server.yaml"],
            ("TOML".to_string(), "heartbeat =\n".to_string()),
            "the text on screen is kept"
        );
        t.stay();
        assert!(t.leaving.get_untracked().is_none());
        assert_eq!(gone.get_untracked(), 0, "Stay goes nowhere");
        assert!(!t.may_leave(move || gone.update(|n| *n += 1)));
        t.leave();
        assert_eq!(gone.get_untracked(), 1, "Leave goes where the user asked");
        assert!(
            t.may_leave(|| {}),
            "the navigation Leave started is let through"
        );
        assert_eq!(
            stuck().question(),
            "server.yaml has TOML text that cannot be saved as YAML yet."
        );
        assert_eq!(
            stuck().reason(),
            "server.yaml cannot be written as YAML yet; fix the text first"
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
