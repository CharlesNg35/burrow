use std::collections::{BTreeMap, BTreeSet};

use burrow_admin_proto::Path;
use burrow_admin_proto::msg::{ConfigFile, FileInfo};
use burrow_admin_proto::ops::Op;

use crate::changeset::Changeset;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileMoves {
    pub places: BTreeMap<Path, String>,
    pub drops: BTreeSet<String>,
}

impl FileMoves {
    pub fn ops(&self) -> Vec<Op> {
        let places = self.places.iter().map(|(path, file)| Op::Place {
            path: path.clone(),
            file: file.clone(),
        });
        let drops = self
            .drops
            .iter()
            .map(|file| Op::DropFile { file: file.clone() });
        places.chain(drops).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.places.is_empty() && self.drops.is_empty()
    }

    pub fn counted(&self, draft: &Changeset) -> usize {
        let added = |p: &Path| match p.as_slice() {
            [section, name] => {
                draft
                    .base()
                    .get(section)
                    .and_then(|s| s.get(name))
                    .is_none()
                    && draft.renamed_from(section, name).is_none()
            }
            _ => false,
        };
        self.places.keys().filter(|p| !added(p)).count() + self.drops.len()
    }

    pub fn place(&mut self, item: &[String], file: &str, now: &str) {
        self.drops.remove(file);
        if file == now {
            self.places.remove(item);
        } else {
            self.places.insert(item.to_vec(), file.to_string());
        }
    }

    pub fn rename(&mut self, from: &[String], to: &[String]) {
        if let Some(file) = self.places.remove(from) {
            self.places.insert(to.to_vec(), file);
        }
    }

    pub fn forget(&mut self, item: &[String]) {
        self.places.remove(item);
    }

    pub fn placed(&self, item: &[String]) -> Option<&str> {
        self.places.get(item).map(String::as_str)
    }
}

pub fn main_name(info: &FileInfo) -> String {
    info.path
        .rsplit(['/', '\\'])
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("server.yaml")
        .to_string()
}

pub fn folder_name(info: &FileInfo) -> String {
    burrow_config::drop_ins::folder_name(&main_name(info))
}

pub fn own_file(info: &FileInfo, item: &str) -> String {
    let safe: String = item
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_matches('-');
    let mut stem = if safe.is_empty() {
        "items".to_string()
    } else {
        safe.to_string()
    };
    if burrow_config::drop_ins::is_device_name(&stem) {
        stem.push_str("-1");
    }
    let folder = folder_name(info);
    let taken = |name: &str| info.files.iter().any(|f| f.name.eq_ignore_ascii_case(name));
    let mut name = format!("{folder}/{stem}.yaml");
    let mut n = 2;
    while taken(&name) {
        name = format!("{folder}/{stem}-{n}.yaml");
        n += 1;
    }
    name
}

pub fn file_of(info: &FileInfo, item: &[String]) -> String {
    info.file_of(item)
        .map_or_else(|| main_name(info), |f| f.name.clone())
}

pub fn locked_file<'a>(info: &'a FileInfo, item: &[String]) -> Option<&'a ConfigFile> {
    info.file_of(item).filter(|f| f.read_only.is_some())
}

pub fn read_only_line(file: &ConfigFile) -> String {
    match &file.read_only {
        Some(why) => format!("Read-only on disk: {} ({why})", file.name),
        None => String::new(),
    }
}

pub fn files_of(info: &FileInfo) -> Vec<ConfigFile> {
    if info.files.is_empty() {
        return vec![ConfigFile {
            name: main_name(info),
            read_only: None,
            items: Vec::new(),
        }];
    }
    info.files.clone()
}

pub const OWN: &str = "*own";

pub fn save_in_options(info: &FileInfo, item: &str) -> Vec<(String, String)> {
    let main = main_name(info);
    let own = own_file(info, item);
    let mut out = vec![(main.clone(), main.clone())];
    for f in info.files.iter().skip(1).filter(|f| f.read_only.is_none()) {
        out.push((f.name.clone(), f.name.clone()));
    }
    if !out.iter().any(|(v, _)| *v == own) {
        out.push((OWN.to_string(), format!("A file of its own ({own})")));
    }
    out
}

pub fn save_in_file(info: &FileInfo, choice: &str, item: &str) -> String {
    match choice {
        OWN => own_file(info, item),
        "" => main_name(info),
        file => file.to_string(),
    }
}

pub fn change_label(change: &burrow_admin_proto::plan::FileChange) -> &'static str {
    use burrow_admin_proto::plan::FileChange;
    match change {
        FileChange::Same => "no change",
        FileChange::Edit => "changed",
        FileChange::Create => "new file",
        FileChange::Remove => "deleted",
    }
}

pub fn empty_offer(file: &str) -> String {
    format!("{file} holds no services, clients or visitors after this change.")
}

pub fn item_count(file: &ConfigFile) -> String {
    match file.items.len() {
        0 => "no items".into(),
        1 => "1 item".into(),
        n => format!("{n} items"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Listed<'a> = (&'a str, Option<&'a str>, &'a [&'a [&'a str]]);

    fn info(files: &[Listed]) -> FileInfo {
        FileInfo {
            path: "/etc/burrow/server/server.yaml".into(),
            files: files
                .iter()
                .map(|(name, ro, items)| ConfigFile {
                    name: name.to_string(),
                    read_only: ro.map(str::to_string),
                    items: items
                        .iter()
                        .map(|i| i.iter().map(|s| s.to_string()).collect())
                        .collect(),
                })
                .collect(),
            ..FileInfo::default()
        }
    }

    fn p(s: &[&str]) -> Path {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn moves_become_place_and_drop_operations() {
        let mut m = FileMoves::default();
        m.place(&p(&["services", "web"]), "server.d/web.yaml", "server.yaml");
        m.drops.insert("server.d/old.yaml".into());
        assert_eq!(
            m.counted(&Changeset::new(
                burrow_admin_proto::serde_json::json!({"services": {"web": {}}})
            )),
            2
        );
        assert_eq!(
            m.ops(),
            [
                Op::Place {
                    path: p(&["services", "web"]),
                    file: "server.d/web.yaml".into()
                },
                Op::DropFile {
                    file: "server.d/old.yaml".into()
                }
            ]
        );
        m.rename(&p(&["services", "web"]), &p(&["services", "www"]));
        assert_eq!(
            m.placed(&p(&["services", "www"])),
            Some("server.d/web.yaml")
        );
        m.place(&p(&["services", "www"]), "server.yaml", "server.yaml");
        assert!(m.places.is_empty(), "back to where it is: no move");
    }

    #[test]
    fn placing_an_item_back_into_a_file_keeps_that_file() {
        let mut m = FileMoves::default();
        let blog = p(&["services", "blog"]);
        m.place(&blog, "server.yaml", "server.d/apps.yaml");
        m.drops.insert("server.d/apps.yaml".into());
        m.place(&blog, "server.d/apps.yaml", "server.d/apps.yaml");
        assert!(
            m.is_empty(),
            "keeping blog in its file undoes the delete: {m:?}"
        );
        m.place(&blog, "server.yaml", "server.d/apps.yaml");
        m.drops.insert("server.d/apps.yaml".into());
        m.place(&p(&["clients", "lab"]), "server.d/apps.yaml", "server.yaml");
        assert!(m.drops.is_empty(), "a new item saved in it keeps it too");
    }

    #[test]
    fn a_new_item_saved_in_a_file_counts_once() {
        let mut c = Changeset::new(
            burrow_admin_proto::serde_json::json!({"services": {"web": {}}, "clients": {"home": {}}}),
        );
        c.set(
            &p(&["clients", "lab"]),
            burrow_admin_proto::serde_json::json!({}),
        );
        assert!(c.rename(&p(&["clients", "home"]), "house"));
        let mut m = FileMoves::default();
        m.place(&p(&["clients", "lab"]), "server.d/lab.yaml", "server.yaml");
        assert_eq!(m.counted(&c), 0, "the new client is counted as its add");
        m.place(
            &p(&["clients", "house"]),
            "server.d/house.yaml",
            "server.yaml",
        );
        m.place(&p(&["services", "web"]), "server.d/web.yaml", "server.yaml");
        assert_eq!(m.counted(&c), 2);
    }

    #[test]
    fn a_new_item_under_a_name_a_swap_freed_counts_as_its_add() {
        let mut c = Changeset::new(
            burrow_admin_proto::serde_json::json!({"services": {"eta2": {}, "web": {}}}),
        );
        for (from, to) in [("eta2", "eps1"), ("web", "eta2"), ("eps1", "web")] {
            assert!(c.rename(&p(&["services", from]), to));
        }
        c.set(
            &p(&["services", "eps1"]),
            burrow_admin_proto::serde_json::json!({}),
        );
        let mut m = FileMoves::default();
        m.place(
            &p(&["services", "eps1"]),
            "server.d/eps1.yaml",
            "server.yaml",
        );
        assert_eq!(m.counted(&c), 0, "the new service is counted as its add");
        m.place(&p(&["services", "web"]), "server.d/web.yaml", "server.yaml");
        assert_eq!(m.counted(&c), 1);
    }

    #[test]
    fn each_item_knows_its_file_and_whether_it_is_read_only() {
        let i = info(&[
            ("server.yaml", None, &[&["services", "web"]]),
            (
                "server.d/tool.yaml",
                Some("it has no write permission"),
                &[&["clients", "box"]],
            ),
            ("server.d/team.yaml", None, &[]),
        ]);
        assert_eq!(
            file_of(&i, &p(&["clients", "box", "secret"])),
            "server.d/tool.yaml"
        );
        assert_eq!(file_of(&i, &p(&["services", "new"])), "server.yaml");
        let locked = locked_file(&i, &p(&["clients", "box"])).unwrap();
        assert_eq!(
            read_only_line(locked),
            "Read-only on disk: server.d/tool.yaml (it has no write permission)"
        );
        assert!(locked_file(&i, &p(&["services", "web"])).is_none());
        let options: Vec<String> = save_in_options(&i, "my blog")
            .into_iter()
            .map(|(v, _)| v)
            .collect();
        assert_eq!(options, ["server.yaml", "server.d/team.yaml", OWN]);
        assert_eq!(save_in_file(&i, OWN, "my blog"), "server.d/my-blog.yaml");
        assert_eq!(save_in_file(&i, "", "x"), "server.yaml");
        assert_eq!(item_count(&i.files[0]), "1 item");
    }

    #[test]
    fn an_own_file_name_is_lower_case_free_and_never_a_device_name() {
        let i = info(&[("server.yaml", None, &[]), ("server.d/web.yaml", None, &[])]);
        assert_eq!(own_file(&i, "Web"), "server.d/web-2.yaml");
        assert_eq!(own_file(&i, "Blog"), "server.d/blog.yaml");
        assert_eq!(own_file(&i, "con"), "server.d/con-1.yaml");
        assert_eq!(own_file(&i, "COM1"), "server.d/com1-1.yaml");
    }

    #[test]
    fn a_server_without_drop_ins_has_one_file_named_after_its_config() {
        let i = info(&[]);
        assert_eq!(main_name(&i), "server.yaml");
        assert_eq!(folder_name(&i), "server.d");
        assert_eq!(files_of(&i).len(), 1);
        assert_eq!(own_file(&i, "web"), "server.d/web.yaml");
    }
}
