use std::path::{Path, PathBuf};

use super::Format;
use super::load::Input;
use super::tree::{self, FileId, Node, ParseError};

const BUNDLE: &str = "#burrow-files 1\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub name: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Docs {
    pub list: Vec<Doc>,
}

impl Docs {
    pub fn one(name: impl Into<String>, text: impl Into<String>) -> Docs {
        Docs {
            list: vec![Doc {
                name: name.into(),
                text: text.into(),
            }],
        }
    }

    pub fn sort(&mut self) {
        if let Some(rest) = self.list.get_mut(1..) {
            rest.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }

    pub fn main(&self) -> &Doc {
        &self.list[0]
    }

    pub fn get(&self, name: &str) -> Option<&Doc> {
        self.list.iter().find(|d| d.name == name)
    }

    pub fn text_of(&self, name: &str) -> Option<&str> {
        self.get(name).map(|d| d.text.as_str())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.list.iter().map(|d| d.name.as_str())
    }

    pub fn inputs(&self, main: &Path, format: Format) -> Vec<Input> {
        self.list
            .iter()
            .enumerate()
            .map(|(i, d)| Input {
                name: if i == 0 {
                    main.display().to_string()
                } else {
                    path_of(main, &d.name)
                        .unwrap_or_default()
                        .display()
                        .to_string()
                },
                text: d.text.clone(),
                format: if i == 0 { format } else { Format::Yaml },
            })
            .collect()
    }

    pub fn tree(&self) -> Result<Node, Vec<ParseError>> {
        let mut trees = Vec::with_capacity(self.list.len());
        for (i, d) in self.list.iter().enumerate() {
            let (_, _, src) = super::patch::split_text(&d.text);
            trees.push(tree::parse_in(
                &src,
                FileId(u16::try_from(i).unwrap_or(u16::MAX)),
            )?);
        }
        Ok(super::load::merge(trees))
    }
}

pub fn folder_of(main: &Path) -> PathBuf {
    main.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

pub fn main_name(main: &Path) -> String {
    main.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "server.yaml".into())
}

pub fn drop_in_folder_name(main: &Path) -> String {
    burrow_config::drop_ins::folder_name(&main_name(main))
}

pub fn drop_in_folder(main: &Path) -> PathBuf {
    folder_of(main).join(drop_in_folder_name(main))
}

pub fn path_of(main: &Path, name: &str) -> Option<PathBuf> {
    if name == main_name(main) {
        return Some(main.to_path_buf());
    }
    let file = burrow_config::drop_ins::file_in(&drop_in_folder_name(main), name)?;
    Some(drop_in_folder(main).join(file))
}

pub fn valid_set(main: &Path, docs: &Docs) -> bool {
    docs.list.first().is_some_and(|d| d.name == main_name(main))
        && docs
            .list
            .iter()
            .skip(1)
            .all(|d| path_of(main, &d.name).is_some())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Skip {
    Extension,
    Name,
    NotRegular,
}

impl Skip {
    pub fn why(self) -> &'static str {
        match self {
            Skip::Extension => "only files ending in .yaml are",
            Skip::Name => "names that are not text, or have : or \\ or control characters, are not",
            Skip::NotRegular => "it is not a regular file",
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Listed {
    pub names: Vec<String>,
    pub ignored: Vec<(String, Skip)>,
}

pub fn list(main: &Path) -> std::io::Result<Option<Listed>> {
    let folder = drop_in_folder(main);
    let entries = match std::fs::read_dir(&folder) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => return Ok(None),
        Err(e) => return Err(e),
    };
    let prefix = drop_in_folder_name(main);
    let mut out = Listed::default();
    for entry in entries {
        let entry = entry?;
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(raw) => {
                let shown = raw.to_string_lossy().into_owned();
                if shown.ends_with(".yaml") {
                    out.ignored.push((format!("{prefix}/{shown}"), Skip::Name));
                }
                continue;
            }
        };
        if name.starts_with('.') || std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        let full = format!("{prefix}/{name}");
        let regular = std::fs::metadata(entry.path()).is_ok_and(|m| m.is_file());
        if !name.ends_with(".yaml") {
            if lower.ends_with(".yml") || lower.ends_with(".json") || lower.ends_with(".yaml") {
                out.ignored.push((full, Skip::Extension));
            }
        } else if burrow_config::drop_ins::file_in(&prefix, &full).is_none() {
            out.ignored.push((full, Skip::Name));
        } else if !regular {
            out.ignored.push((full, Skip::NotRegular));
        } else {
            out.names.push(full);
        }
    }
    out.names.sort();
    out.ignored.sort();
    Ok(Some(out))
}

pub fn read_regular(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let mut file = open.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("it is not a regular file"));
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(text)
}

pub fn read(main: &Path) -> std::io::Result<Docs> {
    let mut docs = Docs::one(main_name(main), std::fs::read_to_string(main)?);
    for name in list(main)?.map(|l| l.names).unwrap_or_default() {
        let Some(path) = path_of(main, &name) else {
            continue;
        };
        let text = read_regular(&path)?;
        docs.list.push(Doc { name, text });
    }
    Ok(docs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnly {
    Marked,
    Link,
    HardLink,
    NotWritable,
}

impl ReadOnly {
    pub fn why(self) -> &'static str {
        match self {
            ReadOnly::Marked if cfg!(windows) => "it has the read-only attribute",
            ReadOnly::Marked => "it has no write permission",
            ReadOnly::Link => "it is a link to another file",
            ReadOnly::HardLink => "it is linked: another file name shares its content",
            ReadOnly::NotWritable => "burrow's user cannot write it",
        }
    }
}

pub fn read_only(path: &Path) -> Option<ReadOnly> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Some(ReadOnly::Link);
    }
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.permissions().mode() & 0o222 == 0 {
            return Some(ReadOnly::Marked);
        }
        if meta.nlink() > 1 {
            return Some(ReadOnly::HardLink);
        }
    }
    #[cfg(not(unix))]
    if meta.permissions().readonly() {
        return Some(ReadOnly::Marked);
    }
    #[cfg(windows)]
    if links(path).is_some_and(|n| n > 1) {
        return Some(ReadOnly::HardLink);
    }
    (!crate::base::access::can_write(path)).then_some(ReadOnly::NotWritable)
}

#[cfg(windows)]
fn links(path: &Path) -> Option<u64> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    winapi_util::file::information(&file)
        .ok()
        .map(|i| i.number_of_links())
}

pub fn clash(main: &Path, name: &str) -> Option<String> {
    let prefix = drop_in_folder_name(main);
    let file = name.strip_prefix(&prefix)?.strip_prefix('/')?;
    let entries = std::fs::read_dir(drop_in_folder(main)).ok()?;
    let listed = list(main)
        .ok()
        .flatten()
        .map(|l| l.names)
        .unwrap_or_default();
    entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|e| e.eq_ignore_ascii_case(file))
        .map(|e| format!("{prefix}/{e}"))
        .find(|e| e != name || !listed.contains(e))
}

pub fn clash_message(name: &str, existing: &str) -> String {
    match existing == name {
        true => burrow_config::drop_ins::exists_message(name),
        false => burrow_config::drop_ins::case_clash_message(name, existing),
    }
}

pub fn open_to_others(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o002 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

impl Docs {
    pub fn bundle(&self) -> String {
        if let [one] = self.list.as_slice() {
            return one.text.clone();
        }
        let mut out = String::from(BUNDLE);
        for d in &self.list {
            out.push_str(&format!("#file {} {}\n", d.text.len(), d.name));
            out.push_str(&d.text);
        }
        out
    }

    pub fn unbundle(main: &Path, body: &str) -> Option<Docs> {
        if !body.starts_with(BUNDLE) {
            return Some(Docs::one(main_name(main), body));
        }
        Docs::split_bundle(body).filter(|d| valid_set(main, d))
    }

    fn split_bundle(body: &str) -> Option<Docs> {
        let mut rest = body.strip_prefix(BUNDLE)?;
        let mut list = Vec::new();
        while !rest.is_empty() {
            let (head, after) = rest.strip_prefix("#file ")?.split_once('\n')?;
            let (len, name) = head.split_once(' ')?;
            let len: usize = len.parse().ok()?;
            let text = after.get(..len)?;
            list.push(Doc {
                name: name.to_string(),
                text: text.to_string(),
            });
            rest = &after[len..];
        }
        (list.len() > 1).then_some(Docs { list })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_of_one_file_is_kept_as_that_file_and_a_larger_set_comes_back_whole() {
        let one = Docs::one("server.yaml", "listen: 1\n");
        assert_eq!(one.bundle(), "listen: 1\n");
        let main = Path::new("/etc/burrow/server.yaml");
        assert_eq!(Docs::unbundle(main, &one.bundle()), Some(one.clone()));
        let mut two = one.clone();
        two.list.push(Doc {
            name: "server.d/a b.yaml".into(),
            text: "services:\n  x: {}\n#file 3 trick\n".into(),
        });
        assert_eq!(Docs::unbundle(main, &two.bundle()), Some(two.clone()));
        let broken = two.bundle().replace("#file 10 ", "#file 99 ");
        assert_eq!(Docs::unbundle(main, &broken), None);
        let mut out = two.clone();
        out.list[1].name = "server.d/../x.yaml".into();
        assert_eq!(Docs::unbundle(main, &out.bundle()), None);
        let mut other_main = two;
        other_main.list[0].name = "other.yaml".into();
        assert_eq!(Docs::unbundle(main, &other_main.bundle()), None);
    }

    #[test]
    fn a_file_with_a_second_name_is_read_only_on_every_system() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.yaml");
        std::fs::write(&file, "services: {}\n").unwrap();
        assert_eq!(read_only(&file), None);
        std::fs::hard_link(&file, dir.path().join("b.yaml")).unwrap();
        assert_eq!(read_only(&file), Some(ReadOnly::HardLink));
        std::fs::remove_file(dir.path().join("b.yaml")).unwrap();
        assert_eq!(read_only(&file), None);
    }
}
