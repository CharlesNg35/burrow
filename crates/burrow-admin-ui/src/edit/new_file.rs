use std::collections::BTreeMap;

use burrow_admin_proto::text_edit::{EditText, TextFile};
use leptos::prelude::*;

use super::TextEdit;

pub fn with_new(
    mut changed: Vec<TextFile>,
    new: &[String],
    values: &BTreeMap<String, String>,
) -> Vec<TextFile> {
    changed.extend(new.iter().filter_map(|name| {
        let text = values.get(name).filter(|t| !t.trim().is_empty())?;
        Some(TextFile {
            name: name.clone(),
            text: text.clone(),
        })
    }));
    changed
}

pub fn empty_new(new: &[String], values: &BTreeMap<String, String>) -> Vec<String> {
    new.iter()
        .filter(|n| values.get(*n).is_none_or(|t| t.trim().is_empty()))
        .cloned()
        .collect()
}

pub fn not_created(name: &str) -> String {
    format!("{name} is empty, so it is not created")
}

pub fn folder_of(text: &EditText) -> String {
    burrow_config::drop_ins::folder_name(
        text.files
            .first()
            .map_or("server.yaml", |f| f.name.as_str()),
    )
}

pub fn new_name(text: &EditText, new: &[String], stem: &str) -> Result<String, String> {
    let folder = folder_of(text);
    let stem = stem.trim().trim_end_matches(".yaml");
    if stem.is_empty() {
        return Err("Type a name for the file".into());
    }
    let file = format!("{folder}/{stem}.yaml");
    let names: Vec<&str> = text
        .files
        .iter()
        .map(|f| f.name.as_str())
        .chain(new.iter().map(String::as_str))
        .collect();
    match burrow_config::drop_ins::new_name_problem(&names, &folder, &file) {
        Some(why) => Err(why),
        None => Ok(file),
    }
}

pub fn kept_new(
    next: &EditText,
    new: &[String],
    typed: &BTreeMap<String, String>,
    values: &mut BTreeMap<String, String>,
) -> Vec<String> {
    let mut still = Vec::new();
    for name in new {
        let mine = typed.get(name).cloned().unwrap_or_default();
        if next.files.iter().any(|f| f.name == *name) {
            if !mine.trim().is_empty() {
                values.insert(name.clone(), mine);
            }
        } else {
            values.insert(name.clone(), mine);
            still.push(name.clone());
        }
    }
    still
}

impl TextEdit {
    pub fn add_file(&self, name: String) {
        self.values.update(|v| {
            v.insert(name.clone(), String::new());
        });
        self.new_files.update(|n| n.push(name.clone()));
        self.open_at(&name, super::Spot::Here);
        self.adding.set(false);
    }

    pub fn is_new(&self, name: &str) -> bool {
        self.new_files
            .with_untracked(|n| n.iter().any(|x| x == name))
    }

    pub fn drop_new(&self, name: &str) {
        self.new_files.update(|n| n.retain(|x| x != name));
        self.values.update(|v| {
            v.remove(name);
        });
        self.unsaved.update(|u| {
            u.remove(name);
        });
        self.drop_tab(name);
    }

    pub fn empty_new_files(&self) -> Vec<String> {
        self.new_files
            .with(|n| self.values.with(|v| empty_new(n, v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::text_edit::EditFile;

    fn text(names: &[&str]) -> EditText {
        EditText {
            files: names
                .iter()
                .map(|n| EditFile {
                    name: n.to_string(),
                    text: "a: 1\n".into(),
                    read_only: None,
                })
                .collect(),
            ..EditText::default()
        }
    }

    fn typed(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_new_file_is_named_by_the_server_rules() {
        let t = text(&["server.yaml", "server.d/apps.yaml"]);
        let new = vec!["server.d/web.yaml".to_string()];
        assert_eq!(new_name(&t, &new, "db"), Ok("server.d/db.yaml".into()));
        assert_eq!(
            new_name(&t, &new, " db.yaml "),
            Ok("server.d/db.yaml".into())
        );
        assert!(new_name(&t, &new, "").is_err());
        assert!(
            new_name(&t, &new, "apps")
                .unwrap_err()
                .contains("already exists")
        );
        assert!(
            new_name(&t, &new, "web")
                .unwrap_err()
                .contains("already exists")
        );
        assert!(
            new_name(&t, &new, "Apps")
                .unwrap_err()
                .contains("upper and lower case")
        );
        assert!(
            new_name(&t, &new, "my apps")
                .unwrap_err()
                .contains("use only letters, digits, dots, dashes and underscores")
        );
        assert!(new_name(&t, &new, "../x").is_err());
        let json = text(&["home.json"]);
        assert_eq!(new_name(&json, &[], "x"), Ok("home.d/x.yaml".into()));
    }

    #[test]
    fn a_new_file_is_sent_once_it_has_text() {
        let new = vec!["server.d/a.yaml".to_string(), "server.d/b.yaml".to_string()];
        let values = typed(&[
            ("server.d/a.yaml", "services: {}\n"),
            ("server.d/b.yaml", "  \n"),
        ]);
        let sent = with_new(Vec::new(), &new, &values);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].name, "server.d/a.yaml");
        assert_eq!(empty_new(&new, &values), ["server.d/b.yaml"]);
        assert_eq!(
            not_created("server.d/b.yaml"),
            "server.d/b.yaml is empty, so it is not created"
        );
    }

    #[test]
    fn a_new_file_survives_a_refetch_until_the_server_has_it() {
        let new = vec!["server.d/a.yaml".to_string(), "server.d/b.yaml".to_string()];
        let mine = typed(&[("server.d/a.yaml", "x: 1\n"), ("server.d/b.yaml", "y: 2\n")]);
        let next = text(&["server.yaml", "server.d/b.yaml"]);
        let mut values = BTreeMap::new();
        let still = kept_new(&next, &new, &mine, &mut values);
        assert_eq!(still, ["server.d/a.yaml"]);
        assert_eq!(values["server.d/a.yaml"], "x: 1\n");
        assert_eq!(
            values["server.d/b.yaml"], "y: 2\n",
            "the text typed for it stays"
        );
    }
}
