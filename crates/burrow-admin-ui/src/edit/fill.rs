use burrow_admin_proto::text_edit::{EditFile, EditText};
use leptos::prelude::*;

use super::TextEdit;
use super::tabs::{MAX_TABS, kept_tabs};

pub fn filled(files: &[EditFile], first: Option<&str>) -> Vec<String> {
    let Some((main, rest)) = files.split_first() else {
        return Vec::new();
    };
    let mut others: Vec<&EditFile> = rest.iter().collect();
    others.sort_by_key(|f| {
        (
            Some(f.name.as_str()) != first,
            std::cmp::Reverse(f.modified),
        )
    });
    std::iter::once(main.name.clone())
        .chain(
            others
                .into_iter()
                .take(MAX_TABS - 1)
                .map(|f| f.name.clone()),
        )
        .collect()
}

pub fn first_active(tabs: &[String], landed: Option<&str>, main: &str) -> String {
    landed
        .filter(|l| tabs.iter().any(|t| t == l))
        .unwrap_or(main)
        .to_string()
}

impl TextEdit {
    pub(super) fn place_tabs(&self, next: &EditText, fresh: bool) {
        let main = next
            .files
            .first()
            .map(|f| f.name.clone())
            .unwrap_or_default();
        let landed = self.landed_file();
        let tabs = if fresh {
            filled(&next.files, landed.as_deref())
        } else {
            self.tabs.with_untracked(|tabs| {
                self.new_files.with_untracked(|n| {
                    kept_tabs(tabs, &main, |t| {
                        next.files.iter().any(|f| f.name == t) || n.iter().any(|x| x == t)
                    })
                })
            })
        };
        if fresh {
            self.active
                .set(first_active(&tabs, landed.as_deref(), &main));
        }
        if self.tabs.with_untracked(|t| *t != tabs) {
            self.tabs.set(tabs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::Spot;
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn at(name: &str, modified: Option<u64>) -> EditFile {
        EditFile {
            name: name.into(),
            text: "a: 1\n".into(),
            read_only: None,
            modified,
        }
    }

    #[test]
    fn tabs_fill_with_the_main_file_first_then_the_newest_files() {
        let files = vec![
            at("server.yaml", Some(1)),
            at("server.d/old.yaml", Some(10)),
            at("server.d/new.yaml", Some(30)),
            at("server.d/mid.yaml", Some(20)),
            at("server.d/unknown.yaml", None),
        ];
        assert_eq!(
            filled(&files, None),
            names(&[
                "server.yaml",
                "server.d/new.yaml",
                "server.d/mid.yaml",
                "server.d/old.yaml",
                "server.d/unknown.yaml"
            ])
        );
        assert_eq!(
            filled(&files, Some("server.d/old.yaml"))[..2],
            names(&["server.yaml", "server.d/old.yaml"]),
            "the file of the last apply comes right after the main file"
        );
        assert_eq!(filled(&files, Some("server.yaml"))[1], "server.d/new.yaml");
        assert!(filled(&[], None).is_empty());
    }

    #[test]
    fn tabs_fill_up_to_ten_and_in_read_order_without_times() {
        let read: Vec<String> = std::iter::once("server.yaml".to_string())
            .chain((1..MAX_TABS + 5).map(|i| format!("server.d/{i}.yaml")))
            .collect();
        let files: Vec<EditFile> = read.iter().map(|n| at(n, None)).collect();
        let got = filled(&files, None);
        assert_eq!(
            got,
            read[..MAX_TABS],
            "an older server: the order it read them, ten at most"
        );
        let mut timed = files.clone();
        timed[14].modified = Some(5);
        let got = filled(&timed, Some("server.d/12.yaml"));
        assert_eq!(got.len(), MAX_TABS);
        assert_eq!(
            got[..4],
            names(&[
                "server.yaml",
                "server.d/12.yaml",
                "server.d/14.yaml",
                "server.d/1.yaml"
            ])
        );
    }

    #[test]
    fn the_shown_tab_is_the_last_applied_file_when_it_is_open() {
        let tabs = names(&["server.yaml", "server.d/a.yaml"]);
        assert_eq!(
            first_active(&tabs, Some("server.d/a.yaml"), "server.yaml"),
            "server.d/a.yaml"
        );
        assert_eq!(
            first_active(&tabs, Some("server.d/gone.yaml"), "server.yaml"),
            "server.yaml"
        );
        assert_eq!(first_active(&tabs, None, "server.yaml"), "server.yaml");
    }

    #[test]
    fn a_fresh_open_fills_the_tabs_and_a_closed_tab_stays_closed_on_a_new_text() {
        let text = EditText {
            files: vec![
                at("server.yaml", Some(1)),
                at("server.d/a.yaml", Some(2)),
                at("server.d/b.yaml", Some(3)),
            ],
            ..EditText::default()
        };
        let t = TextEdit::new();
        t.place_tabs(&text, true);
        assert_eq!(
            t.tabs.get_untracked(),
            names(&["server.yaml", "server.d/b.yaml", "server.d/a.yaml"])
        );
        assert_eq!(t.active.get_untracked(), "server.yaml");
        assert!(
            t.recent.get_untracked().is_empty(),
            "filled tabs are not used yet"
        );
        t.loaded.set(Some(Ok(std::sync::Arc::new(text.clone()))));
        t.close_tab("server.d/b.yaml", false);
        t.place_tabs(&text, false);
        assert_eq!(
            t.tabs.get_untracked(),
            names(&["server.yaml", "server.d/a.yaml"]),
            "the user closed it"
        );
        t.landed.set(Some(super::super::landed::Landed {
            file: "server.d/a.yaml".into(),
            others: 0,
            path: vec!["clients".into(), "lab".into()],
        }));
        t.place_tabs(&text, true);
        assert_eq!(
            t.tabs.get_untracked(),
            names(&["server.yaml", "server.d/a.yaml", "server.d/b.yaml"])
        );
        assert_eq!(
            t.active.get_untracked(),
            "server.d/a.yaml",
            "the last apply's file"
        );
        t.reveal_landed();
        assert!(
            t.spot.get_untracked().is_none(),
            "only Open goes to the item"
        );
        t.reveal.set_value(true);
        t.reveal_landed();
        assert_eq!(
            t.spot.get_untracked(),
            Some((
                "server.d/a.yaml".into(),
                Spot::Path(vec!["clients".into(), "lab".into()])
            ))
        );
        assert!(!t.reveal.get_value());
    }
}
