use burrow_admin_proto::Path;
use burrow_admin_proto::plan::{ChangeKind, FileChange, Plan};
use leptos::prelude::*;

use super::{Spot, TextEdit};
use crate::store::Store;

const TOAST_TEXT: usize = 74;
const MIN_NAME: usize = 10;

#[derive(Clone, Debug, PartialEq)]
pub struct Landed {
    pub file: String,
    pub others: usize,
    pub path: Path,
}

impl Landed {
    pub fn place(&self, room: usize) -> String {
        let more = match self.others {
            0 => String::new(),
            1 => " and 1 more file".into(),
            n => format!(" and {n} more files"),
        };
        let room = room.saturating_sub(more.chars().count());
        let base = self.file.rsplit('/').next().unwrap_or(&self.file);
        let name = if self.file.chars().count() <= room {
            self.file.clone()
        } else {
            crate::format::fit(base, room.max(MIN_NAME))
        };
        format!("{name}{more}")
    }

    pub fn spot(&self) -> Spot {
        if self.path.is_empty() {
            Spot::Here
        } else {
            Spot::Path(self.path.clone())
        }
    }
}

pub fn landed(plan: &Plan) -> Option<Landed> {
    let main = plan.files.first()?.file.as_str();
    let touched: Vec<&_> = plan
        .files
        .iter()
        .filter(|f| f.change != FileChange::Same)
        .collect();
    let file = touched
        .iter()
        .find(|f| f.file != main && matches!(f.change, FileChange::Edit | FileChange::Create))?;
    let path = plan
        .changes
        .iter()
        .filter(|c| c.kind != ChangeKind::Remove)
        .map(|c| c.part_of.clone().unwrap_or_else(|| c.path.clone()))
        .find(|p| !p.is_empty())
        .unwrap_or_default();
    Some(Landed {
        file: file.file.clone(),
        others: touched.len() - 1,
        path,
    })
}

pub fn applied_note(version: &str, landed: Option<&Landed>) -> String {
    let head = format!("Applied: now the {version}");
    match landed {
        None => head,
        Some(l) => {
            let lead = format!("{head}, saved in ");
            let room = TOAST_TEXT.saturating_sub(lead.chars().count());
            format!("{lead}{}", l.place(room))
        }
    }
}

impl TextEdit {
    pub fn landed_file(&self) -> Option<String> {
        self.landed
            .with_untracked(|l| l.as_ref().map(|l| l.file.clone()))
    }

    pub(super) fn reveal_landed(&self) {
        if !self.reveal.get_value() {
            return;
        }
        let Some(l) = self.landed.get_untracked() else {
            self.reveal.set_value(false);
            return;
        };
        let there =
            untrack(|| self.text()).is_some_and(|t| t.files.iter().any(|f| f.name == l.file));
        if there {
            self.reveal.set_value(false);
            self.open_at(&l.file, l.spot());
        }
    }
}

pub fn open_landed(s: Store) {
    let (to, query) = crate::route::file_tab();
    crate::route::go_query(to, query);
    let t = s.text;
    if t.landed.with_untracked(Option::is_none) {
        return;
    }
    if t.open.get_untracked() {
        t.reveal.set_value(true);
        t.reveal_landed();
        return;
    }
    if untrack(|| s.edit.dirty()) {
        return;
    }
    t.start();
    t.reveal.set_value(true);
}

#[cfg(test)]
mod tests {
    use burrow_admin_proto::plan::{Change, FileDiff};

    use super::*;

    fn diff(file: &str, change: FileChange) -> FileDiff {
        FileDiff {
            file: file.into(),
            change,
            diff: Default::default(),
            left_empty: None,
        }
    }

    fn plan(files: Vec<FileDiff>) -> Plan {
        Plan {
            files,
            changes: vec![Change {
                kind: ChangeKind::Add,
                path: vec!["clients".into(), "lab".into()],
                summary: "adds client lab".into(),
                part_of: None,
            }],
            ..Plan::default()
        }
    }

    #[test]
    fn a_change_in_a_drop_in_lands_there_and_one_in_the_main_file_nowhere() {
        let p = plan(vec![
            diff("server.yaml", FileChange::Same),
            diff("server.d/clients.yaml", FileChange::Create),
        ]);
        let l = landed(&p).unwrap();
        assert_eq!(l.file, "server.d/clients.yaml");
        assert_eq!(l.others, 0);
        assert_eq!(l.spot(), Spot::Path(vec!["clients".into(), "lab".into()]));
        let main_only = plan(vec![
            diff("server.yaml", FileChange::Edit),
            diff("server.d/a.yaml", FileChange::Same),
        ]);
        assert_eq!(landed(&main_only), None);
        assert_eq!(landed(&plan(Vec::new())), None, "a single file has no list");
        let gone = plan(vec![
            diff("server.yaml", FileChange::Same),
            diff("server.d/a.yaml", FileChange::Remove),
        ]);
        assert_eq!(landed(&gone), None, "a deleted file cannot be opened");
    }

    #[test]
    fn of_several_changed_files_the_first_drop_in_is_named() {
        let p = plan(vec![
            diff("server.yaml", FileChange::Edit),
            diff("server.d/a.yaml", FileChange::Same),
            diff("server.d/b.yaml", FileChange::Remove),
            diff("server.d/c.yaml", FileChange::Edit),
            diff("server.d/d.yaml", FileChange::Create),
        ]);
        let l = landed(&p).unwrap();
        assert_eq!(l.file, "server.d/c.yaml");
        assert_eq!(l.others, 3);
        assert_eq!(l.place(80), "server.d/c.yaml and 3 more files");
    }

    #[test]
    fn the_applied_note_names_the_file_only_beyond_the_main_one() {
        let v = "version of 14:02";
        assert_eq!(applied_note(v, None), "Applied: now the version of 14:02");
        let mut l = Landed {
            file: "server.d/clients.yaml".into(),
            others: 0,
            path: Vec::new(),
        };
        assert_eq!(
            applied_note(v, Some(&l)),
            "Applied: now the version of 14:02, saved in server.d/clients.yaml"
        );
        assert_eq!(l.spot(), Spot::Here);
        l.others = 1;
        assert_eq!(
            applied_note(v, Some(&l)),
            "Applied: now the version of 14:02, saved in clients.yaml and 1 more file"
        );
        l.others = 2;
        assert!(applied_note(v, Some(&l)).ends_with(" saved in clients.yaml and 2 more files"));
        l.file = "server.d/a-very-long-drop-in-file-name-for-the-lab-clients.yaml".into();
        let long = applied_note("version of 19:09:33", Some(&l));
        assert!(long.chars().count() <= TOAST_TEXT, "{long}");
        assert!(long.ends_with("yaml and 2 more files"), "{long}");
        assert!(!long.contains("server.d/") && long.contains('…'), "{long}");
        l.file = "server.d/lab-clients-floor-two.yaml".into();
        l.others = 0;
        assert_eq!(
            applied_note(v, Some(&l)),
            "Applied: now the version of 14:02, saved in lab-clients-floor-two.yaml",
            "the folder goes first"
        );
    }
}
