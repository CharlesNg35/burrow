use burrow_admin_proto::plan::{Diff, Hunk};
use burrow_admin_proto::text_edit::placeholder;

use std::cell::RefCell;
use std::collections::HashMap;

use super::mask::{self, Masked, Named};
use crate::admin::core::{Core, diff, text, values};
use crate::config::Format;
use crate::config::docs::Docs;
use crate::config::patch::Rename;

pub struct Shown<'a> {
    core: &'a Core,
    main: String,
    fresh: Vec<String>,
    renames: &'a [Rename],
    cache: RefCell<HashMap<(Side, String), Option<Masked>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Side {
    Old,
    New,
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
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn ensure(&self, side: Side, name: &str, text: &str) {
        let key = (side, name.to_string());
        if self.cache.borrow().contains_key(&key) {
            return;
        }
        let format = match name == self.main {
            true => self.core.store.format(),
            false => Format::Yaml,
        };
        let renames = match side {
            Side::Old => self.renames,
            Side::New => &[],
        };
        let named = Named { name, format };
        let root = &self.core.schema().root;
        let ids = &self.core.text_edit.ids;
        let m = mask::mask_renamed(named, text, root, ids, renames, &mut mask::Taken::new()).ok();
        self.cache.borrow_mut().insert(key, m);
    }

    fn masked(&self, side: Side, name: &str, text: &str) -> Option<String> {
        self.ensure(side, name, text);
        let cache = self.cache.borrow();
        cache[&(side, name.to_string())]
            .as_ref()
            .map(|m| m.text.clone())
    }

    pub fn hidden(&self, old: &Docs, new: &Docs) -> values::Hidden {
        for (side, docs) in [(Side::Old, old), (Side::New, new)] {
            docs.list
                .iter()
                .for_each(|d| self.ensure(side, &d.name, &d.text));
        }
        let cache = self.cache.borrow();
        let parts = cache
            .values()
            .flatten()
            .map(|m| (&m.tree, m.fields.as_slice()));
        values::Hidden::of_trees(parts, &self.core.schema().root)
    }

    fn line(&self, line: &str) -> String {
        let mut out = text::mask_tokens(line, &[]);
        for fp in &self.fresh {
            out = out.replace(&placeholder(fp), &format!("«new secret {fp}»"));
        }
        out
    }

    pub fn diff(&self, name: &str, old: &str, new: &str) -> Diff {
        match (
            self.masked(Side::Old, name, old),
            self.masked(Side::New, name, new),
        ) {
            (Some(a), Some(b)) => diff::unified(&a, &b, &|l| self.line(l)),
            _ => not_shown(name),
        }
    }
}
