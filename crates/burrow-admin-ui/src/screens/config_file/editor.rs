use std::collections::BTreeMap;
use std::sync::Arc;

use burrow_admin_proto::diag::Diag;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::text_edit::{EditFile, EditText, PLACEHOLDER_PATTERN};
use burrow_ui_kit::Icon;
use config_editor_leptos::{
    ConfigDocuments, Document, DocumentsHandle, EditorSettings, EditorTheme, Language, SaveRequest,
    Schema, SchemaSource, SecretOptions, Severity, check_syntax, flush_pending_edits,
};
use leptos::prelude::*;

use super::problems::host_problems;

pub const SECRET_HINT: &str = "The current secret, kept as it is. Type a new value to replace it.";

pub struct EditorProps {
    pub text: Signal<Option<Arc<EditText>>>,
    pub values: Signal<BTreeMap<String, String>>,
    pub problems: Signal<Vec<Diag>>,
    pub on_change: Callback<(String, String)>,
    pub on_review: Callback<()>,
    pub unsavable: RwSignal<Option<String>>,
    pub flush: StoredValue<Option<Callback<()>>>,
}

fn schema_of(schema: &Value) -> Option<SchemaSource> {
    if schema.is_null() {
        return None;
    }
    let json = schema.to_string();
    Some(match Schema::compile(&json) {
        Ok(s) => SchemaSource::Compiled(s),
        Err(_) => SchemaSource::Json(json),
    })
}

fn language_of(name: &str) -> Language {
    Language::from_path(name).unwrap_or(Language::YAML)
}

pub fn syntax_problem(files: &[EditFile], values: &BTreeMap<String, String>) -> Option<String> {
    files.iter().find_map(|f| {
        let now = values.get(&f.name).filter(|v| **v != f.text)?;
        let d = check_syntax(language_of(&f.name), now)
            .into_iter()
            .find(|d| d.severity == Severity::Error)?;
        Some(match d.line {
            Some(line) => format!(
                "{} has a syntax error on line {line}: {}",
                f.name, d.message
            ),
            None => format!("{} has a syntax error: {}", f.name, d.message),
        })
    })
}

fn documents(
    t: &EditText,
    values: &BTreeMap<String, String>,
    problems: &[Diag],
    schema: Option<&SchemaSource>,
    secrets: &SecretOptions,
) -> Vec<Document> {
    let main = t.files.first().map(|f| f.name.as_str()).unwrap_or_default();
    t.files
        .iter()
        .map(|f| {
            let now = values.get(&f.name).unwrap_or(&f.text).clone();
            let d = Document::new(f.name.clone(), f.name.clone(), now)
                .baseline(f.text.clone())
                .read_only(f.read_only.is_some())
                .secrets(secrets.clone())
                .host_diagnostics(host_problems(problems, &f.name, main));
            match schema {
                Some(s) => d.schema(s.clone()),
                None => d,
            }
        })
        .collect()
}

fn read_only_note(text: Signal<Option<Arc<EditText>>>, active: RwSignal<String>) -> impl IntoView {
    move || {
        let (name, why) = text.with(|t| {
            let t = t.as_ref()?;
            active
                .with(|a| t.files.iter().find(|f| f.name == *a))
                .and_then(|f| Some((f.name.clone(), f.read_only.clone()?)))
        })?;
        Some(view! {
            <p class="mb-2 flex min-w-0 items-start gap-1.5 text-xs text-text-muted [overflow-wrap:anywhere]" data-testid="file-read-only">
                <Icon name="lock" small=true/>
                <span>{format!("{name} is read-only: {why}")}</span>
            </p>
        })
    }
}

pub fn text_editor(p: EditorProps) -> AnyView {
    let EditorProps {
        text,
        values,
        problems,
        on_change,
        on_review,
        unsavable,
        flush,
    } = p;
    flush.set_value(Some(Callback::new(|_| flush_pending_edits())));
    let first = text.get_untracked();
    let schema = first.as_ref().and_then(|t| schema_of(&t.schema));
    let pattern = first
        .as_ref()
        .map(|t| t.placeholder.clone())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| PLACEHOLDER_PATTERN.to_string());
    let secrets = SecretOptions::default()
        .with_placeholder(pattern)
        .with_hint(SECRET_HINT);
    let main = first
        .as_ref()
        .and_then(|t| t.files.first().map(|f| f.name.clone()))
        .unwrap_or_default();
    let active = RwSignal::new(main.clone());
    let list = Signal::derive(move || {
        text.with(|t| {
            t.as_ref().map_or_else(Vec::new, |t| {
                values.with(|v| problems.with(|ps| documents(t, v, ps, schema.as_ref(), &secrets)))
            })
        })
    });
    Effect::new(move |_| {
        let why = text.with(|t| {
            t.as_ref()
                .and_then(|t| values.with(|v| syntax_problem(&t.files, v)))
        });
        if unsavable.with_untracked(|u| *u != why) {
            unsavable.set(why);
        }
    });
    let settings = EditorSettings {
        theme: EditorTheme::ViraInspired,
        show_footer: true,
        ..EditorSettings::default()
    };
    let tabs = DocumentsHandle::new();
    view! {
        <div class="min-w-0 w-full" data-testid="file-editor">
            {read_only_note(text, active)}
            <ConfigDocuments
                documents=list
                initial_active=main
                handle=tabs
                label="Config files"
                settings=Signal::stored(settings)
                on_select=Callback::new(move |id: String| active.set(id))
                on_change=Callback::new(move |change: (String, String)| on_change.run(change))
                on_save=Callback::new(move |_: (String, SaveRequest)| on_review.run(()))
            />
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, text: &str) -> EditFile {
        EditFile {
            name: name.into(),
            text: text.into(),
            read_only: None,
        }
    }

    #[test]
    fn a_syntax_error_in_an_edited_file_holds_the_review() {
        let files = vec![
            file("server.yaml", "a: 1\n"),
            file("server.d/x.yaml", "b: [\n"),
        ];
        let mut values = BTreeMap::new();
        values.insert("server.d/x.yaml".to_string(), "b: [\n".to_string());
        assert_eq!(
            syntax_problem(&files, &values),
            None,
            "an untouched file is the server's"
        );
        values.insert("server.yaml".to_string(), "a: [1\n".to_string());
        let why = syntax_problem(&files, &values).expect("a syntax error");
        assert!(why.starts_with("server.yaml has a syntax error"), "{why}");
        values.insert("server.yaml".to_string(), "a: 2\n".to_string());
        assert_eq!(syntax_problem(&files, &values), None);
        let json = vec![file("server.json", "{}")];
        let mut v = BTreeMap::new();
        v.insert("server.json".to_string(), "{\"a\": }".to_string());
        assert!(syntax_problem(&json, &v).is_some());
    }

    #[test]
    fn each_file_is_one_tab_with_its_server_text_as_baseline() {
        let t = EditText {
            rev: "b2s:1".into(),
            files: vec![
                file("server.yaml", "a: 1\n"),
                EditFile {
                    read_only: Some("not writable by burrow".into()),
                    ..file("server.d/ro.yaml", "c: 1\n")
                },
            ],
            ..EditText::default()
        };
        let mut values = BTreeMap::new();
        values.insert("server.yaml".to_string(), "a: 2\n".to_string());
        let docs = documents(&t, &values, &[], None, &SecretOptions::default());
        assert_eq!(docs.len(), 2);
        assert_eq!(
            (docs[0].id.as_str(), docs[0].value.as_str()),
            ("server.yaml", "a: 2\n")
        );
        assert_eq!(docs[0].baseline.as_deref(), Some("a: 1\n"));
        assert!(!docs[0].read_only && docs[1].read_only);
        assert_eq!(docs[1].value, "c: 1\n");
    }
}
