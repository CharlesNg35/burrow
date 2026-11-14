use leptos::prelude::*;

use super::TextEdit;

pub const MAX_TABS: usize = 10;

#[derive(Clone, Debug, PartialEq)]
pub enum Spot {
    Here,
    Path(Vec<String>),
    Pointer(String),
    Range(u32, u32, u32),
}

pub fn used(recent: &[String], name: &str) -> Vec<String> {
    std::iter::once(name.to_string())
        .chain(recent.iter().filter(|n| *n != name).cloned())
        .collect()
}

pub fn opened(
    tabs: &[String],
    recent: &[String],
    name: &str,
    main: &str,
    dirty: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut out = tabs.to_vec();
    if !out.iter().any(|t| t == name) {
        out.push(name.to_string());
    }
    while out.len() > MAX_TABS {
        let rank = |n: &str| recent.iter().position(|r| r == n).unwrap_or(usize::MAX);
        let oldest = out
            .iter()
            .enumerate()
            .filter(|(_, t)| *t != main && *t != name && !dirty(t))
            .max_by_key(|(i, t)| (rank(t), std::cmp::Reverse(*i)))
            .map(|(_, t)| t.clone());
        match oldest {
            Some(o) => out.retain(|t| *t != o),
            None => break,
        }
    }
    out
}

pub fn active_after(previous: &[String], current: &[String], active: &str) -> String {
    let listed = |id: &str| current.iter().any(|c| c == id);
    if listed(active) {
        return active.to_string();
    }
    if let Some(i) = previous.iter().position(|id| id == active) {
        let after = previous[i + 1..].iter();
        let before = previous[..i].iter().rev();
        if let Some(n) = after.chain(before).find(|id| listed(id)) {
            return n.clone();
        }
    }
    current.first().cloned().unwrap_or_default()
}

pub fn kept_tabs(tabs: &[String], main: &str, exists: impl Fn(&str) -> bool) -> Vec<String> {
    std::iter::once(main.to_string())
        .chain(tabs.iter().filter(|t| *t != main && exists(t)).cloned())
        .collect()
}

impl TextEdit {
    pub fn main_name(&self) -> String {
        self.loaded.with_untracked(|l| {
            l.as_ref()
                .and_then(|r| r.as_ref().ok())
                .and_then(|t| t.files.first().map(|f| f.name.clone()))
                .unwrap_or_default()
        })
    }

    pub fn file_dirty(&self, name: &str) -> bool {
        if self.unsaved.with_untracked(|u| u.contains(name)) {
            return true;
        }
        let now = self.values.with_untracked(|v| v.get(name).cloned());
        if self.is_new(name) {
            return now.is_some_and(|t| !t.trim().is_empty());
        }
        let server = self.loaded.with_untracked(|l| {
            l.as_ref().and_then(|r| r.as_ref().ok()).and_then(|t| {
                t.files
                    .iter()
                    .find(|f| f.name == name)
                    .map(|f| f.text.clone())
            })
        });
        matches!((now, server), (Some(n), Some(s)) if n != s)
    }

    fn keeps_tab(&self, name: &str) -> bool {
        self.is_new(name) || self.file_dirty(name)
    }

    pub fn open_file(&self, name: &str) {
        let main = self.main_name();
        let next = self.tabs.with_untracked(|tabs| {
            self.recent
                .with_untracked(|r| opened(tabs, r, name, &main, |n| self.keeps_tab(n)))
        });
        if self.tabs.with_untracked(|t| *t != next) {
            self.tabs.set(next);
        }
        self.recent.update(|r| *r = used(r, name));
        self.active.set(name.to_string());
    }

    pub fn used(&self, name: &str) {
        if self
            .recent
            .with_untracked(|r| r.first().map(String::as_str) != Some(name))
        {
            self.recent.update(|r| *r = used(r, name));
        }
    }

    pub fn open_at(&self, name: &str, spot: Spot) {
        self.open_file(name);
        self.spot.set(Some((name.to_string(), spot)));
    }

    pub fn close_tab(&self, name: &str, editor_dirty: bool) {
        if name == self.main_name() {
            return;
        }
        let asks = if self.is_new(name) {
            self.file_dirty(name)
        } else {
            editor_dirty || self.file_dirty(name)
        };
        if asks {
            self.closing.set(Some(name.to_string()));
        } else if self.is_new(name) {
            self.drop_new(name);
        } else {
            self.drop_tab(name);
        }
    }

    pub fn discard_tab(&self, name: &str) {
        self.forget(name);
        if self.is_new(name) {
            self.drop_new(name);
            return;
        }
        let server = self.text().and_then(|t| {
            t.files
                .iter()
                .find(|f| f.name == name)
                .map(|f| f.text.clone())
        });
        if let Some(text) = server {
            self.set_value(name, text);
        }
        for map in [self.edit_as, self.shown] {
            map.update(|m| {
                m.remove(name);
            });
        }
        self.unsaved.update(|u| {
            u.remove(name);
        });
        self.drop_tab(name);
    }

    pub(super) fn drop_tab(&self, name: &str) {
        let before = self.tabs.get_untracked();
        let after: Vec<String> = before.iter().filter(|t| *t != name).cloned().collect();
        self.tabs.set(after.clone());
        let now = self
            .active
            .with_untracked(|a| active_after(&before, &after, a));
        if self.active.with_untracked(|a| *a != now) {
            self.active.set(now);
        }
    }

    pub fn set_unsaved(&self, name: &str, dirty: bool) {
        if dirty && self.lost.with_untracked(Option::is_some) {
            self.lost.set(None);
        }
        if self.unsaved.with_untracked(|u| u.contains(name) == dirty) {
            return;
        }
        self.unsaved.update(|u| {
            if dirty {
                u.insert(name.to_string());
            } else {
                u.remove(name);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn many(n: usize) -> Vec<String> {
        std::iter::once("server.yaml".to_string())
            .chain((1..n).map(|i| format!("server.d/{i}.yaml")))
            .collect()
    }

    #[test]
    fn opening_a_tab_adds_it_once_at_the_end() {
        let tabs = names(&["server.yaml"]);
        let got = opened(&tabs, &[], "server.d/a.yaml", "server.yaml", |_| false);
        assert_eq!(got, names(&["server.yaml", "server.d/a.yaml"]));
        assert_eq!(
            opened(&got, &[], "server.d/a.yaml", "server.yaml", |_| false),
            got
        );
        assert_eq!(
            used(&names(&["b", "a", "c"]), "a"),
            names(&["a", "b", "c"]),
            "the file used last comes first"
        );
    }

    #[test]
    fn an_eleventh_tab_closes_the_least_recently_used_clean_one() {
        let tabs = many(MAX_TABS);
        let mut recent: Vec<String> = tabs.iter().rev().cloned().collect();
        recent = used(&recent, "server.d/1.yaml");
        let got = opened(&tabs, &recent, "server.d/new.yaml", "server.yaml", |_| {
            false
        });
        assert_eq!(got.len(), MAX_TABS);
        assert!(got.contains(&"server.d/1.yaml".to_string()), "used lately");
        assert!(
            !got.contains(&"server.d/2.yaml".to_string()),
            "the oldest clean tab goes: {got:?}"
        );
        assert_eq!(got.last().unwrap(), "server.d/new.yaml");
    }

    #[test]
    fn the_main_file_and_dirty_tabs_are_never_closed_for_a_new_one() {
        let tabs = many(MAX_TABS);
        let got = opened(&tabs, &[], "server.d/new.yaml", "server.yaml", |n| {
            n == "server.d/1.yaml"
        });
        assert_eq!(got[0], "server.yaml");
        assert!(got.contains(&"server.d/1.yaml".to_string()));
        assert!(
            !got.contains(&"server.d/2.yaml".to_string()),
            "never used: the earliest opened goes"
        );
        let all_dirty = opened(&tabs, &[], "server.d/new.yaml", "server.yaml", |n| {
            n != "server.yaml"
        });
        assert_eq!(
            all_dirty.len(),
            MAX_TABS + 1,
            "nothing with unsaved edits is closed, so one more stays open"
        );
    }

    #[test]
    fn closing_the_shown_tab_shows_its_neighbor_as_the_editor_does() {
        let before = names(&["m", "a", "b"]);
        assert_eq!(active_after(&before, &names(&["m", "b"]), "a"), "b");
        assert_eq!(active_after(&before, &names(&["m", "a"]), "b"), "a");
        assert_eq!(active_after(&before, &names(&["m", "b"]), "m"), "m");
        assert_eq!(active_after(&before, &names(&["m"]), "gone"), "m");
    }

    #[test]
    fn a_new_server_text_keeps_open_tabs_of_files_that_still_exist() {
        let tabs = names(&["server.yaml", "server.d/a.yaml", "server.d/gone.yaml"]);
        let got = kept_tabs(&tabs, "server.yaml", |n| n != "server.d/gone.yaml");
        assert_eq!(got, names(&["server.yaml", "server.d/a.yaml"]));
        assert_eq!(
            kept_tabs(&[], "server.yaml", |_| true),
            names(&["server.yaml"])
        );
    }

    #[test]
    fn a_tab_closes_at_once_when_clean_and_asks_when_it_has_unsaved_edits() {
        use burrow_admin_proto::text_edit::{EditFile, EditText};
        let t = TextEdit::new();
        let file = |n: &str| EditFile {
            name: n.into(),
            text: "a: 1\n".into(),
            read_only: None,
        };
        t.loaded.set(Some(Ok(std::sync::Arc::new(EditText {
            files: vec![file("server.yaml"), file("server.d/a.yaml")],
            ..EditText::default()
        }))));
        t.tabs.set(names(&["server.yaml"]));
        t.open_file("server.d/a.yaml");
        assert_eq!(
            t.tabs.get_untracked(),
            names(&["server.yaml", "server.d/a.yaml"])
        );
        assert_eq!(t.active.get_untracked(), "server.d/a.yaml");
        t.close_tab("server.yaml", false);
        assert_eq!(t.tabs.get_untracked().len(), 2, "the main file stays open");
        t.close_tab("server.d/a.yaml", false);
        assert_eq!(t.tabs.get_untracked(), names(&["server.yaml"]));
        assert_eq!(t.active.get_untracked(), "server.yaml");
        assert!(t.closing.get_untracked().is_none());
        t.open_file("server.d/a.yaml");
        t.close_tab("server.d/a.yaml", true);
        assert_eq!(
            t.closing.get_untracked().as_deref(),
            Some("server.d/a.yaml")
        );
        t.closing.set(None);
        t.values.update(|v| {
            v.insert("server.d/a.yaml".into(), "a: 2\n".into());
        });
        assert!(t.file_dirty("server.d/a.yaml"));
        t.close_tab("server.d/a.yaml", false);
        assert_eq!(
            t.closing.get_untracked().as_deref(),
            Some("server.d/a.yaml"),
            "an edit kept in TextEdit asks too"
        );
        assert_eq!(t.recent.get_untracked()[0], "server.d/a.yaml");
    }

    #[test]
    fn a_tab_picked_in_the_strip_counts_as_used() {
        use burrow_admin_proto::text_edit::{EditFile, EditText};
        let t = TextEdit::new();
        let files: Vec<String> = many(MAX_TABS + 1);
        t.loaded.set(Some(Ok(std::sync::Arc::new(EditText {
            files: files
                .iter()
                .map(|n| EditFile {
                    name: n.clone(),
                    text: "a: 1\n".into(),
                    read_only: None,
                })
                .collect(),
            ..EditText::default()
        }))));
        t.tabs.set(names(&["server.yaml"]));
        for f in &files[1..MAX_TABS] {
            t.open_file(f);
        }
        t.used("server.d/1.yaml");
        t.open_file(&files[MAX_TABS]);
        let open = t.tabs.get_untracked();
        assert!(
            open.contains(&"server.d/1.yaml".to_string()),
            "just used: stays"
        );
        assert!(
            !open.contains(&"server.d/2.yaml".to_string()),
            "the oldest unused closes: {open:?}"
        );
    }

    #[test]
    fn the_ten_tab_rule_never_closes_a_new_file() {
        use burrow_admin_proto::text_edit::{EditFile, EditText};
        let t = TextEdit::new();
        let files: Vec<String> = many(MAX_TABS + 1);
        t.loaded.set(Some(Ok(std::sync::Arc::new(EditText {
            files: files
                .iter()
                .map(|n| EditFile {
                    name: n.clone(),
                    text: "a: 1\n".into(),
                    read_only: None,
                })
                .collect(),
            ..EditText::default()
        }))));
        t.tabs.set(names(&["server.yaml"]));
        t.new_files.set(names(&["server.d/new.yaml"]));
        t.values.update(|v| {
            v.insert("server.d/new.yaml".into(), String::new());
        });
        t.open_file("server.d/new.yaml");
        for f in &files[1..] {
            t.open_file(f);
        }
        let open = t.tabs.get_untracked();
        assert_eq!(open.len(), MAX_TABS);
        assert!(
            open.contains(&"server.d/new.yaml".to_string()),
            "an empty new file keeps its tab until the user closes it: {open:?}"
        );
    }
}
