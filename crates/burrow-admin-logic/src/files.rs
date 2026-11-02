use std::collections::{BTreeMap, BTreeSet};

use burrow_admin_proto::Path;
use burrow_admin_proto::msg::{ConfigFile, FileInfo};
use burrow_admin_proto::ops::Op;

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

    pub fn len(&self) -> usize {
        self.places.len() + self.drops.len()
    }

    pub fn place(&mut self, item: &[String], file: &str, now: &str) {
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
    let main = main_name(info);
    let stem = main.rsplit_once('.').map_or(main.as_str(), |(s, _)| s);
    format!("{stem}.d")
}

pub fn own_file(info: &FileInfo, item: &str) -> String {
    let safe: String = item
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_matches('-');
    let safe = if safe.is_empty() { "items" } else { safe };
    format!("{}/{safe}.yaml", folder_name(info))
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
        assert_eq!(m.len(), 2);
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
    fn a_server_without_drop_ins_has_one_file_named_after_its_config() {
        let i = info(&[]);
        assert_eq!(main_name(&i), "server.yaml");
        assert_eq!(folder_name(&i), "server.d");
        assert_eq!(files_of(&i).len(), 1);
        assert_eq!(own_file(&i, "web"), "server.d/web.yaml");
    }
}
