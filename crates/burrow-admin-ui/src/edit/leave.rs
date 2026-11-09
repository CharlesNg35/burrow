use std::collections::BTreeMap;
use std::rc::Rc;

use leptos::prelude::*;

use super::TextEdit;
pub use super::unwritten::Unwritten;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Went {
    Now,
    Later,
    Not,
}

pub type Then = Rc<dyn Fn() -> Went>;

fn names(all: &[Unwritten]) -> Vec<String> {
    all.iter().map(|u| u.name.clone()).collect()
}

impl TextEdit {
    fn live(&self) -> Option<Vec<Unwritten>> {
        if !self.open.get_untracked() {
            return Some(Vec::new());
        }
        self.before_review
            .try_with_value(|h| h.as_ref().map(|h| h()))
            .flatten()
    }

    pub fn unwritten(&self) -> Vec<Unwritten> {
        match self.live() {
            Some(now) => {
                self.hold(&now);
                now
            }
            None => self.held.with_untracked(|h| h.values().cloned().collect()),
        }
    }

    fn hold(&self, now: &[Unwritten]) {
        let map: BTreeMap<String, Unwritten> =
            now.iter().map(|u| (u.name.clone(), u.clone())).collect();
        if self.held.with_untracked(|h| *h != map) {
            self.held.set(map);
        }
    }

    pub fn note_unwritten(&self) {
        let _ = self.unwritten();
    }

    pub fn note_unwritten_on_unmount(&self) {
        let Some(now) = self.live() else {
            return;
        };
        let fresh: Vec<Unwritten> = self
            .reported
            .with_value(|r| now.into_iter().filter(|u| !r.contains(&u.name)).collect());
        if !fresh.is_empty() {
            self.held.update(|h| {
                for u in fresh {
                    h.insert(u.name.clone(), u);
                }
            });
        }
    }

    pub fn lose_unwritten(&self) {
        let mut all = self.held.try_update(std::mem::take).unwrap_or_default();
        if let Some(now) = self.live() {
            for u in now {
                all.insert(u.name.clone(), u);
            }
        }
        let all: Vec<Unwritten> = self
            .reported
            .with_value(|r| all.into_values().filter(|u| !r.contains(&u.name)).collect());
        if let Some(note) = super::unwritten::lost_note(&all) {
            for u in &all {
                self.forget(&u.name);
            }
            self.reported.update_value(|r| r.extend(names(&all)));
            self.lost.set(Some(note));
        }
    }

    pub fn may_leave(&self, then: impl Fn() -> Went + 'static) -> bool {
        let all = self.unwritten();
        if all.is_empty() {
            return true;
        }
        self.pending_pop.set_value(None);
        self.then.set_value(Some(Rc::new(then)));
        self.leaving.set(all);
        false
    }

    pub fn stay(&self) {
        self.then.set_value(None);
        self.pending_pop.set_value(None);
        if self.leaving.with_untracked(|l| !l.is_empty()) {
            self.leaving.set(Vec::new());
        }
    }

    pub fn leave(&self) {
        let all = self.leaving.get_untracked();
        if all.is_empty() {
            return;
        }
        self.leaving.set(Vec::new());
        let then = self.then.try_update_value(Option::take).flatten();
        match then.map_or(Went::Not, |then| then()) {
            Went::Now => {
                for n in names(&all) {
                    self.forget(&n);
                }
            }
            Went::Later => self.pending_pop.set_value(Some(names(&all))),
            Went::Not => {}
        }
    }

    pub fn popped(&self) -> bool {
        match self.pending_pop.try_update_value(Option::take).flatten() {
            Some(all) => {
                for n in all {
                    self.forget(&n);
                }
                true
            }
            None => false,
        }
    }

    pub fn drop_pending_pop(&self) {
        self.pending_pop.set_value(None);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stuck(name: &str, shown: &str) -> Unwritten {
        Unwritten {
            name: name.into(),
            shown: shown.into(),
            own: "YAML".into(),
            text: "broken = [\n".into(),
        }
    }

    fn edited(t: TextEdit, names: &[&str]) {
        t.unsaved
            .update(|u| u.extend(names.iter().map(|n| n.to_string())));
    }

    fn screen(t: TextEdit, now: Vec<Unwritten>) {
        t.before_review
            .set_value(Some(Rc::new(move || now.clone())));
    }

    #[test]
    fn leaving_asks_for_every_file_and_forgets_exactly_those() {
        let t = TextEdit::new();
        t.open.set(true);
        edited(t, &["server.yaml", "server.d/a.yaml", "server.d/b.yaml"]);
        let gone = RwSignal::new(0);
        let go = move || {
            gone.update(|n| *n += 1);
            Went::Now
        };
        screen(t, Vec::new());
        assert!(t.may_leave(go), "nothing held: leave at once");
        screen(
            t,
            vec![
                stuck("server.yaml", "TOML"),
                stuck("server.d/a.yaml", "JSON"),
            ],
        );
        assert!(!t.may_leave(go));
        assert_eq!(t.leaving.get_untracked().len(), 2, "the dialog names both");
        assert_eq!(t.held.get_untracked().len(), 2);
        t.stay();
        assert!(t.leaving.get_untracked().is_empty());
        assert_eq!(gone.get_untracked(), 0, "Stay goes nowhere");
        assert!(!t.may_leave(go));
        t.leave();
        assert_eq!(gone.get_untracked(), 1, "Leave goes where the user asked");
        assert_eq!(
            t.unsaved.get_untracked(),
            ["server.d/b.yaml".to_string()].into(),
            "both files go with the navigation, the other edit stays"
        );
        assert!(t.held.get_untracked().is_empty());
        assert!(!t.may_leave(go), "the next exit asks again");
    }

    #[test]
    fn a_leave_that_does_not_navigate_keeps_the_edits_and_lets_nothing_through() {
        let t = TextEdit::new();
        t.open.set(true);
        edited(t, &["server.yaml"]);
        screen(t, vec![stuck("server.yaml", "TOML")]);
        assert!(!t.may_leave(|| Went::Not));
        t.leave();
        assert!(t.unsaved.get_untracked().contains("server.yaml"));
        assert!(!t.popped(), "no pop is let through");
        assert!(!t.may_leave(|| Went::Later));
        t.leave();
        assert!(t.unsaved.get_untracked().contains("server.yaml"));
        assert!(t.popped(), "the Back it started is let through once");
        assert!(t.unsaved.get_untracked().is_empty());
        assert!(!t.popped(), "only once");
    }

    #[test]
    fn closing_one_tab_drops_only_its_file() {
        let t = TextEdit::new();
        t.open.set(true);
        screen(
            t,
            vec![
                stuck("server.d/a.yaml", "TOML"),
                stuck("server.d/b.yaml", "JSON"),
            ],
        );
        t.note_unwritten();
        t.forget("server.d/a.yaml");
        assert_eq!(
            t.held.get_untracked().keys().cloned().collect::<Vec<_>>(),
            ["server.d/b.yaml"]
        );
    }

    fn editor_mounted(t: TextEdit, now: Vec<Unwritten>) -> Owner {
        let editor = Owner::new();
        editor.with(|| {
            screen(t, now);
            on_cleanup(move || {
                t.note_unwritten_on_unmount();
                let _ = t.before_review.try_set_value(None);
            });
        });
        editor
    }

    const LOST_TWO: &str = "The server changed; text that could not be saved yet is gone in server.d/a.yaml and server.yaml";

    #[test]
    fn a_server_change_names_the_lost_text_when_the_editor_unmounts_first() {
        let t = TextEdit::new();
        t.open.set(true);
        edited(t, &["server.yaml", "server.d/a.yaml"]);
        let editor = editor_mounted(t, vec![stuck("server.yaml", "TOML")]);
        t.note_unwritten();
        screen(
            t,
            vec![
                stuck("server.yaml", "TOML"),
                stuck("server.d/a.yaml", "JSON"),
            ],
        );
        editor.cleanup();
        assert!(
            t.before_review.with_value(Option::is_none),
            "the editor's hook is gone before the screen looks"
        );
        t.lose_unwritten();
        assert_eq!(
            t.lost.get_untracked().as_deref(),
            Some(LOST_TWO),
            "the last keystrokes count too"
        );
        assert!(t.unsaved.get_untracked().is_empty());
        assert!(t.held.get_untracked().is_empty());
    }

    #[test]
    fn a_server_change_names_the_lost_text_when_it_is_seen_before_the_unmount() {
        let t = TextEdit::new();
        t.open.set(true);
        edited(t, &["server.yaml", "server.d/a.yaml"]);
        let editor = editor_mounted(t, vec![stuck("server.yaml", "TOML")]);
        t.note_unwritten();
        screen(
            t,
            vec![
                stuck("server.yaml", "TOML"),
                stuck("server.d/a.yaml", "JSON"),
            ],
        );
        t.lose_unwritten();
        assert_eq!(
            t.lost.get_untracked().as_deref(),
            Some(LOST_TWO),
            "read from the editors that are still there"
        );
        editor.cleanup();
        assert!(
            t.held.get_untracked().is_empty(),
            "the unmount does not hold what was already reported"
        );
        t.lost.set(None);
        t.lose_unwritten();
        assert!(t.lost.get_untracked().is_none(), "no second note");
        t.set_unsaved("server.yaml", true);
        assert!(t.unsaved.get_untracked().contains("server.yaml"));
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
