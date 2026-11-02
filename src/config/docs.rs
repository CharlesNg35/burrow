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
        let folder = folder_of(main);
        self.list
            .iter()
            .enumerate()
            .map(|(i, d)| Input {
                name: if i == 0 {
                    main.display().to_string()
                } else {
                    folder.join(&d.name).display().to_string()
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
