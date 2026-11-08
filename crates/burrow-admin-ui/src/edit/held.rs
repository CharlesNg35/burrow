use leptos::prelude::*;

use super::Edit;

pub const TEXT_FIRST: &str = "Review or cancel your Config file text edits first";

impl Edit {
    fn inside_open_edit(&self, path: &[String]) -> bool {
        self.editing
            .with_untracked(|m| m.keys().any(|k| path.starts_with(k)))
    }

    pub(super) fn held(&self, path: Option<&[String]>) -> bool {
        if !self.text_held.get_untracked() {
            return false;
        }
        if path.is_some_and(|p| self.inside_open_edit(p)) {
            return false;
        }
        if let Some(s) = crate::store::try_store() {
            s.text_first();
        }
        true
    }

    pub(super) fn held_edit(&self, entity: &[String]) -> bool {
        untrack(|| self.unsaved(entity)) && self.held(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    fn p(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn unreviewed_text_edits_hold_every_addition_to_the_changes() {
        let text = RwSignal::new(true);
        let e = Edit::held_by(text.into());
        e.sync_base(
            &json!({"visitors": {"lap": {"max_connections": 5}}}),
            "b2s:1",
        );
        let lap = p(&["visitors", "lap"]);
        let max = p(&["visitors", "lap", "max_connections"]);
        e.set(&max, Some(json!(6)));
        e.remove_pruned(&lap);
        e.add_secret(&p(&["visitors", "lap", "secret"]), "h", "fp");
        e.set_text(&p(&["geo"]), Some("x: 1".into()));
        e.drop_file("server.d/x.yaml", true);
        assert!(
            !e.dirty(),
            "nothing outside an open edit reaches the changes"
        );
        e.start_edit(&lap);
        e.draft.update(|d| d.set(&max, json!(7)));
        assert_eq!(
            e.value(&max),
            Some(json!(7)),
            "typing in an open edit still works"
        );
        e.stop_edit(&lap);
        assert!(e.is_editing(&lap), "Add to changes does not add");
        assert_eq!(e.pending(), 0);
        e.draft.update(|d| d.set(&max, json!(5)));
        e.stop_edit(&lap);
        assert!(
            !e.is_editing(&lap),
            "closing an edit with nothing changed is never held"
        );
        text.set(false);
        e.start_edit(&lap);
        e.draft.update(|d| d.set(&max, json!(8)));
        e.stop_edit(&lap);
        assert!(!e.is_editing(&lap));
        assert_eq!(
            e.pending(),
            1,
            "once the text is reviewed or cancelled, it adds"
        );
    }
}
