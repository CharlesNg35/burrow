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
                    path_of(main, &d.name).display().to_string()
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
    let stem = main
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "server".into());
    format!("{stem}.d")
}

pub fn drop_in_folder(main: &Path) -> PathBuf {
    folder_of(main).join(drop_in_folder_name(main))
}

pub fn path_of(main: &Path, name: &str) -> PathBuf {
    if name == main_name(main) {
        return main.to_path_buf();
    }
    name.split('/')
        .fold(folder_of(main), |p, part| p.join(part))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Listed {
    pub names: Vec<String>,
    pub ignored: Vec<String>,
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
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') || std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if name.ends_with(".yaml") {
            out.names.push(format!("{prefix}/{name}"));
        } else if lower.ends_with(".yml") || lower.ends_with(".json") || lower.ends_with(".yaml") {
            out.ignored.push(format!("{prefix}/{name}"));
        }
    }
    out.names.sort();
    out.ignored.sort();
    Ok(Some(out))
}

pub fn read(main: &Path) -> std::io::Result<Docs> {
    let mut docs = Docs::one(main_name(main), std::fs::read_to_string(main)?);
    for name in list(main)?.map(|l| l.names).unwrap_or_default() {
        let text = std::fs::read_to_string(path_of(main, &name))?;
        docs.list.push(Doc { name, text });
    }
    Ok(docs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnly {
    Marked,
    NotWritable,
}

impl ReadOnly {
    pub fn why(self) -> &'static str {
        match self {
            ReadOnly::Marked if cfg!(windows) => "it has the read-only attribute",
            ReadOnly::Marked => "it has no write permission",
            ReadOnly::NotWritable => "burrow's user cannot write it",
        }
    }
}

pub fn read_only(path: &Path) -> Option<ReadOnly> {
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o222 == 0 {
            return Some(ReadOnly::Marked);
        }
    }
    #[cfg(not(unix))]
    if meta.permissions().readonly() {
        return Some(ReadOnly::Marked);
    }
    (!crate::base::access::can_write(path)).then_some(ReadOnly::NotWritable)
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

    pub fn unbundle(main: &str, body: &str) -> Docs {
        Docs::split_bundle(body).unwrap_or_else(|| Docs::one(main, body))
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
        assert_eq!(Docs::unbundle("server.yaml", &one.bundle()), one);
        let mut two = one.clone();
        two.list.push(Doc {
            name: "server.d/a b.yaml".into(),
            text: "services:\n  x: {}\n#file 3 trick\n".into(),
        });
        assert_eq!(Docs::unbundle("server.yaml", &two.bundle()), two);
        let broken = two.bundle().replace("#file 10 ", "#file 99 ");
        assert_eq!(Docs::unbundle("server.yaml", &broken).list.len(), 1);
    }
}
