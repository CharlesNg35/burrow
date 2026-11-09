use burrow_admin_proto::plan::{Diff, Hunk};
use burrow_admin_proto::text_edit::placeholder;

use super::mask::{self, Named};
use crate::admin::core::{Core, diff, text};
use crate::config::Format;
use crate::config::patch::Rename;

pub struct Shown<'a> {
    core: &'a Core,
    main: String,
    fresh: Vec<String>,
    renames: &'a [Rename],
}

pub fn not_shown(name: &str) -> Diff {
    let line = format!("({name} does not parse; its diff is not shown)");
    Diff {
        unified: format!("{line}\n"),
        hunks: vec![Hunk {
            old_start: 0,
            old_lines: 0,
            new_start: 0,
            new_lines: 0,
            lines: vec![(" ".into(), line)],
        }],
    }
}

impl<'a> Shown<'a> {
    pub fn new(core: &'a Core, fresh: &[(String, String)], renames: &'a [Rename]) -> Shown<'a> {
        Shown {
            core,
            main: crate::config::docs::main_name(core.store.path()),
            fresh: fresh.iter().map(|(_, fp)| fp.clone()).collect(),
            renames,
        }
    }

    fn masked(&self, name: &str, text: &str, renames: &[Rename]) -> Option<String> {
        let format = match name == self.main {
            true => self.core.store.format(),
            false => Format::Yaml,
        };
        let named = Named { name, format };
        let root = &self.core.schema().root;
        mask::mask_renamed(named, text, root, &self.core.text_edit.ids, renames)
            .ok()
            .map(|m| m.text)
    }

    fn line(&self, line: &str) -> String {
        let mut out = text::mask_secrets(line, &[]);
        for fp in &self.fresh {
            out = out.replace(&placeholder(fp), &format!("«new secret {fp}»"));
        }
        out
    }

    pub fn diff(&self, name: &str, old: &str, new: &str) -> Diff {
        match (
            self.masked(name, old, self.renames),
            self.masked(name, new, &[]),
        ) {
            (Some(a), Some(b)) => diff::unified(&a, &b, &|l| self.line(l)),
            _ => not_shown(name),
        }
    }
}
