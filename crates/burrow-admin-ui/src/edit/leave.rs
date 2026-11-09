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

    pub fn lost(&self) -> String {
        format!(
            "The server changed; the {} text in {} that could not be saved as {} yet is gone",
            self.shown, self.name, self.own
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Went {
    Now,
    Later,
    Not,
}

pub type Then = Rc<dyn Fn() -> Went>;

impl TextEdit {
    pub fn unwritten(&self) -> Option<Unwritten> {
        if !self.open.get_untracked() {
            return None;
        }
        self.before_review
            .try_with_value(|h| h.as_ref().and_then(|h| h()))
            .flatten()
    }

    pub fn may_leave(&self, then: impl Fn() -> Went + 'static) -> bool {
        let Some(u) = self.unwritten() else {
            return true;
        };
        self.pending_pop.set_value(None);
        self.then.set_value(Some(Rc::new(then)));
        self.leaving.set(Some(u));
        false
    }

    pub fn stay(&self) {
        self.then.set_value(None);
        self.pending_pop.set_value(None);
        if self.leaving.with_untracked(Option::is_some) {
            self.leaving.set(None);
        }
    }

    pub fn leave(&self) {
        let Some(u) = self.leaving.get_untracked() else {
            return;
        };
        self.leaving.set(None);
        let then = self.then.try_update_value(Option::take).flatten();
        match then.map_or(Went::Not, |then| then()) {
            Went::Now => {
                self.forget(&u.name);
                self.held.set(None);
            }
            Went::Later => self.pending_pop.set_value(Some(u.name)),
            Went::Not => {}
        }
    }

    pub fn popped(&self) -> bool {
        match self.pending_pop.try_update_value(Option::take).flatten() {
            Some(name) => {
                self.forget(&name);
                self.held.set(None);
                true
            }
            None => false,
        }
    }

    pub fn note_unwritten(&self) {
        let now = self.unwritten();
        if self.held.with_untracked(|h| *h != now) {
            self.held.set(now);
        }
    }

    pub fn note_unwritten_on_unmount(&self) {
        if self.lost.with_untracked(Option::is_some) {
            return;
        }
        if let Some(u) = self.unwritten() {
            self.held.set(Some(u));
        }
    }

    pub fn lose_unwritten(&self) {
        if let Some(u) = self.held.try_update(Option::take).flatten() {
            self.forget(&u.name);
            self.lost.set(Some(u.lost()));
        }
    }

    pub fn drop_pending_pop(&self) {
        self.pending_pop.set_value(None);
    }

    fn forget(&self, name: &str) {
        self.unsaved.update(|s| {
            s.remove(name);
        });
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
        t.unsaved.update(|u| {
            u.insert("server.yaml".into());
        });
        let gone = RwSignal::new(0);
        let go = move || {
            gone.update(|n| *n += 1);
            Went::Now
        };
        assert!(t.may_leave(go), "nothing held: leave at once");
        t.before_review.set_value(Some(Rc::new(|| Some(stuck()))));
        assert!(!t.may_leave(go));
        assert_eq!(t.leaving.get_untracked(), Some(stuck()));
        t.stay();
        assert!(t.leaving.get_untracked().is_none());
        assert_eq!(gone.get_untracked(), 0, "Stay goes nowhere");
        assert!(!t.may_leave(go));
        t.leave();
        assert_eq!(gone.get_untracked(), 1, "Leave goes where the user asked");
        assert!(
            t.unsaved.get_untracked().is_empty(),
            "the file's unsaved state goes with the navigation"
        );
        assert!(!t.may_leave(go), "the next exit asks again");
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
    fn a_leave_that_does_not_navigate_keeps_the_edit_and_lets_nothing_through() {
        let t = TextEdit::new();
        t.open.set(true);
        t.unsaved.update(|u| {
            u.insert("server.yaml".into());
        });
        t.before_review.set_value(Some(Rc::new(|| Some(stuck()))));
        assert!(!t.may_leave(|| Went::Not));
        t.leave();
        assert!(
            t.unsaved.get_untracked().contains("server.yaml"),
            "nothing navigated, so the edit is still a change"
        );
        assert!(!t.popped(), "no pop is let through");
        assert!(!t.may_leave(|| Went::Now), "the next exit asks");
        assert!(!t.may_leave(|| Went::Later));
        t.leave();
        assert!(
            t.unsaved.get_untracked().contains("server.yaml"),
            "a Back that has not happened yet keeps the edit"
        );
        assert!(t.popped(), "the Back it started is let through once");
        assert!(t.unsaved.get_untracked().is_empty());
        assert!(!t.popped(), "only once");
    }

    #[test]
    fn a_server_change_says_the_unsaved_edit_as_text_is_gone() {
        let t = TextEdit::new();
        t.open.set(true);
        t.unsaved.update(|u| {
            u.insert("server.yaml".into());
        });
        let editor = Owner::new();
        editor.with(|| {
            t.before_review.set_value(Some(Rc::new(|| Some(stuck()))));
            on_cleanup(move || {
                t.note_unwritten_on_unmount();
                let _ = t.before_review.try_set_value(None);
            });
        });
        t.note_unwritten();
        editor.cleanup();
        assert!(
            t.before_review.with_value(Option::is_none),
            "as on a remount: the editor's hook is gone before the screen looks"
        );
        t.lose_unwritten();
        assert_eq!(
            t.lost.get_untracked().as_deref(),
            Some(
                "The server changed; the TOML text in server.yaml that could not be saved as YAML yet is gone"
            )
        );
        assert!(t.unsaved.get_untracked().is_empty());
        assert!(t.held.get_untracked().is_none());
        t.lose_unwritten();
        t.set_unsaved("server.yaml", true);
        assert!(
            t.lost.get_untracked().is_none(),
            "typing again clears the note"
        );
        let quiet = TextEdit::new();
        quiet.open.set(true);
        quiet.lose_unwritten();
        assert!(
            quiet.lost.get_untracked().is_none(),
            "nothing held, nothing to say"
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
