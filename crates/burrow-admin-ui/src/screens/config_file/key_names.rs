use std::collections::BTreeMap;

use burrow_admin_proto::text_edit::{DROP_IN_SCHEMA_ID, EditText, SCHEMA_ID, pointer};
use leptos::prelude::*;
use leptos_config_editor::{GoToDefinition, KeyName, KeyNames, Language, tools::keys_at};

use crate::edit::{Spot, TextEdit};

fn closed<'a>(
    t: &'a EditText,
    values: &'a BTreeMap<String, String>,
    open: &'a [String],
) -> Vec<(String, String)> {
    let saved = t
        .files
        .iter()
        .map(|f| (&f.name, values.get(&f.name).unwrap_or(&f.text)));
    let new = values
        .iter()
        .filter(|(n, _)| !t.files.iter().any(|f| f.name == **n));
    saved
        .chain(new)
        .filter(|(n, _)| !open.contains(n))
        .map(|(n, text)| (n.clone(), text.clone()))
        .collect()
}

fn names(main: &str, files: &[(String, String)], maps: &[String]) -> Vec<KeyNames> {
    files
        .iter()
        .flat_map(|(name, text)| {
            let language = Language::from_path(name).unwrap_or(Language::YAML);
            let schema = if name == main {
                SCHEMA_ID
            } else {
                DROP_IN_SCHEMA_ID
            };
            maps.iter().filter_map(move |at| {
                let keys = keys_at(text, language, at)?;
                let found = keys.into_iter().map(|k| KeyName::new(k).with_source(name));
                Some(KeyNames::new(at, found).for_schema(schema))
            })
        })
        .filter(|n| !n.names.is_empty())
        .collect()
}

pub fn host_names(
    text: Signal<Option<std::sync::Arc<EditText>>>,
    values: Signal<BTreeMap<String, String>>,
    open: Signal<Vec<String>>,
) -> Signal<Vec<KeyNames>> {
    let files = Memo::new(move |_| {
        text.with(|t| {
            t.as_ref().map_or_else(Vec::new, |t| {
                values.with(|v| open.with(|o| closed(t, v, o)))
            })
        })
    });
    let found = Memo::new(move |_| {
        text.with(|t| {
            t.as_ref().map_or_else(Vec::new, |t| {
                let main = t.files.first().map(|f| f.name.as_str()).unwrap_or_default();
                files.with(|f| names(main, f, &t.key_maps))
            })
        })
    });
    found.into()
}

pub fn go_to(edit: TextEdit) -> Callback<GoToDefinition> {
    Callback::new(move |g: GoToDefinition| {
        if let Some(file) = g.source {
            let at = format!("{}{}", g.pointer, pointer(&[g.name]));
            edit.open_at(&file, Spot::Pointer(at));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::text_edit::EditFile;

    fn file(name: &str, text: &str) -> EditFile {
        EditFile {
            name: name.into(),
            text: text.into(),
            read_only: None,
        }
    }

    #[test]
    fn the_files_not_open_give_their_names_unsaved_edits_included() {
        let t = EditText {
            files: vec![
                file("server.yaml", "clients:\n  home: {}\n"),
                file(
                    "server.d/nas.yaml",
                    "clients:\n  nas: {}\nvisitors:\n  laptop: {}\n",
                ),
                file("server.d/web.yaml", "services: {}\n"),
            ],
            key_maps: vec!["/clients".into(), "/visitors".into()],
            ..EditText::default()
        };
        let mut values = BTreeMap::new();
        values.insert(
            "server.d/web.yaml".to_string(),
            "clients:\n  edge: {}\n".to_string(),
        );
        values.insert(
            "server.d/new.yaml".to_string(),
            "clients:\n  fresh: {}\n".to_string(),
        );
        let open = vec!["server.yaml".to_string()];
        let files = closed(&t, &values, &open);
        assert_eq!(
            files.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            [
                "server.d/nas.yaml",
                "server.d/web.yaml",
                "server.d/new.yaml"
            ],
            "an open file's names come live from its tab"
        );
        let found = names("server.yaml", &files, &t.key_maps);
        type Named<'a> = Vec<(&'a str, Option<&'a str>)>;
        let got: Vec<(Option<&str>, &str, Named)> = found
            .iter()
            .map(|n| {
                let each = n
                    .names
                    .iter()
                    .map(|k| (k.name.as_str(), k.source.as_deref()));
                (n.schema.as_deref(), n.pointer.as_str(), each.collect())
            })
            .collect();
        let d = Some(DROP_IN_SCHEMA_ID);
        assert_eq!(
            got,
            [
                (d, "/clients", vec![("nas", Some("server.d/nas.yaml"))]),
                (d, "/visitors", vec![("laptop", Some("server.d/nas.yaml"))]),
                (d, "/clients", vec![("edge", Some("server.d/web.yaml"))]),
                (d, "/clients", vec![("fresh", Some("server.d/new.yaml"))]),
            ]
        );
        let main = names(
            "server.yaml",
            &[("server.yaml".into(), "clients:\n  a: {}\n".into())],
            &t.key_maps,
        );
        assert_eq!(main[0].schema.as_deref(), Some(SCHEMA_ID));
        let broken = names(
            "server.yaml",
            &[("server.d/x.yaml".into(), "clients: [\n".into())],
            &t.key_maps,
        );
        assert!(
            broken.is_empty(),
            "a file that does not parse gives no names"
        );
    }
}
