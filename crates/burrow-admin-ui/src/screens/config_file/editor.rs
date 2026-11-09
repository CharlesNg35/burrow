use std::collections::BTreeMap;
use std::sync::Arc;

use burrow_admin_proto::diag::Diag;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::text_edit::{EditText, PLACEHOLDER_PATTERN};
use config_editor_leptos::{
    CloseRequest, ConfigDocuments, Diagnostic, DiagnosticOrigin, Document, DocumentsHandle,
    EditorSettings, EditorTheme, Language, SaveRequest, Schema, SchemaSource, SecretOptions,
    Severity, flush_pending_edits,
};
use leptos::prelude::*;

use crate::edit::{Spot, Unwritten};

use super::edit_as::own_label;
use super::follow::follow;
use super::problems::host_problems;

const HELD_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

pub const SECRET_HINT: &str = "The current secret, kept as it is. Type a new value to replace it.";

#[derive(Clone, Copy)]
pub struct EditorProps {
    pub text: Signal<Option<Arc<EditText>>>,
    pub values: Signal<BTreeMap<String, String>>,
    pub problems: Signal<Vec<Diag>>,
    pub on_change: Callback<(String, String)>,
    pub on_review: Callback<()>,
    pub before_review: StoredValue<Option<crate::edit::Hook>, LocalStorage>,
    pub active: RwSignal<String>,
    pub edit_as: RwSignal<BTreeMap<String, String>>,
    pub on_add: Callback<()>,
    pub on_close: Callback<(String, bool)>,
    pub on_dirty: Callback<(String, bool)>,
    pub tabs: Signal<Vec<String>>,
    pub spot: RwSignal<Option<(String, Spot)>>,
    pub edit: crate::edit::TextEdit,
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

pub fn first_error(name: &str, problems: &[Diagnostic]) -> Option<String> {
    let d = problems
        .iter()
        .find(|d| d.severity == Severity::Error && d.origin == DiagnosticOrigin::Editor)?;
    Some(match d.line {
        Some(line) => format!("{name} has an error on line {line}: {}", d.message),
        None => format!("{name} has an error: {}", d.message),
    })
}

#[derive(Default)]
struct Schemas {
    main: Option<SchemaSource>,
    drop_in: Option<SchemaSource>,
}

impl Schemas {
    fn of(t: &EditText) -> Schemas {
        Schemas {
            main: schema_of(&t.schema),
            drop_in: t.drop_in_schema.as_ref().and_then(schema_of),
        }
    }

    fn for_file(&self, main: bool) -> Option<&SchemaSource> {
        if main {
            self.main.as_ref()
        } else {
            self.drop_in.as_ref().or(self.main.as_ref())
        }
    }
}

struct Kept<'a> {
    asked: &'a BTreeMap<String, String>,
    held: &'a BTreeMap<String, Unwritten>,
}

fn documents(
    t: &EditText,
    values: &BTreeMap<String, String>,
    problems: &[Diag],
    schemas: &Schemas,
    secrets: &SecretOptions,
    kept: Kept,
    open: &[String],
) -> Vec<Document> {
    let Kept { asked, held } = kept;
    let main = t.files.first().map(|f| f.name.as_str()).unwrap_or_default();
    open.iter()
        .filter_map(|name| {
            let d = match t.files.iter().find(|f| f.name == *name) {
                Some(f) => {
                    let now = values.get(&f.name).unwrap_or(&f.text).clone();
                    let d = Document::new(f.name.clone(), f.name.clone(), now)
                        .baseline(f.text.clone())
                        .closable(f.name != main);
                    match &f.read_only {
                        Some(why) => d.read_only_reason(why.clone()),
                        None => d,
                    }
                }
                None => {
                    let now = values.get(name)?.clone();
                    Document::new(name.clone(), name.clone(), now)
                        .baseline(String::new())
                        .closable(true)
                }
            };
            let d = d
                .secrets(secrets.clone())
                .host_diagnostics(host_problems(problems, name, main));
            let kept = held
                .get(name)
                .and_then(|u| Some((requested(&u.own, Some(&u.shown))?, u.text.clone())));
            let d = match (
                kept,
                requested(own_label(name), asked.get(name).map(String::as_str)),
            ) {
                (Some((l, text)), _) => d.edit_as_text(l, text),
                (None, Some(l)) => d.edit_as(l),
                (None, None) => d,
            };
            Some(match schemas.for_file(name == main) {
                Some(s) => d.schema(s.clone()),
                None => d,
            })
        })
        .collect()
}

pub fn first_shown(names: &[String], active: &str) -> String {
    names
        .iter()
        .find(|n| *n == active)
        .or_else(|| names.first())
        .cloned()
        .unwrap_or_default()
}

pub fn language(label: &str) -> Option<Language> {
    match label {
        "YAML" => Some(Language::YAML),
        "JSON" => Some(Language::JSON),
        "TOML" => Some(Language::TOML),
        _ => None,
    }
}

pub fn requested(own: &str, asked: Option<&str>) -> Option<Language> {
    asked.filter(|a| *a != own).and_then(language)
}

fn label_of(l: Language) -> String {
    l.label().to_string()
}

#[leptos::wasm_split::wasm_split(config_editor, wasm_split_path = ::leptos::wasm_split, fallible)]
pub fn text_editor(p: EditorProps) -> Result<AnyView, burrow_ui_kit::LoadFailed> {
    let EditorProps {
        text,
        values,
        problems,
        on_change,
        on_review,
        before_review,
        active,
        edit_as,
        on_add,
        on_close,
        on_dirty,
        tabs: open,
        spot,
        edit,
    } = p;
    let first = text.get_untracked();
    let schemas = first.as_ref().map(|t| Schemas::of(t)).unwrap_or_default();
    let pattern = first
        .as_ref()
        .map(|t| t.placeholder.clone())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| PLACEHOLDER_PATTERN.to_string());
    let secrets = SecretOptions::default()
        .with_placeholder(pattern)
        .with_hint(SECRET_HINT);
    let main = first_shown(&open.get_untracked(), &active.get_untracked());
    active.set(main.clone());
    let held = edit.held;
    let list = Signal::derive(move || {
        text.with(|t| {
            t.as_ref().map_or_else(Vec::new, |t| {
                values.with(|v| {
                    problems.with(|ps| {
                        edit_as.with(|a| {
                            held.with(|h| {
                                open.with(|o| {
                                    documents(
                                        t,
                                        v,
                                        ps,
                                        &schemas,
                                        &secrets,
                                        Kept { asked: a, held: h },
                                        o,
                                    )
                                })
                            })
                        })
                    })
                })
            })
        })
    });
    let settings = EditorSettings {
        theme: EditorTheme::ViraInspired,
        show_footer: true,
        styled: true,
        ..EditorSettings::default()
    };
    let tabs = DocumentsHandle::new();
    let keep = move || {
        let names: Vec<String> = held.with_untracked(|h| h.keys().cloned().collect());
        for name in names {
            if let Some(now) = tabs.editor(&name).and_then(|e| e.text()) {
                edit.keep_text(&name, now);
            }
        }
    };
    let typed = StoredValue::new(0u64);
    let keep_soon = move || {
        typed.update_value(|n| *n += 1);
        let n = typed.get_value();
        set_timeout(
            move || {
                if typed.try_get_value() == Some(n) {
                    keep();
                }
            },
            HELD_AFTER,
        );
    };
    before_review.set_value(Some(std::rc::Rc::new(move || {
        flush_pending_edits();
        keep();
    })));
    edit.ask
        .set_value(Some(std::rc::Rc::new(move |name: &str, label: &str| {
            if let Some(l) = language(label) {
                tabs.edit_as(name, l);
            }
        })));
    on_cleanup(move || {
        let _ = before_review.try_set_value(None);
        let _ = edit.ask.try_set_value(None);
    });
    follow(active, spot, tabs);
    Ok(view! {
        <div class="min-w-0 w-full" data-testid="file-editor" on:input=move |_| keep_soon() on:paste=move |_| keep_soon()>
            <ConfigDocuments
                documents=list
                initial_active=main
                handle=tabs
                label="Config files"
                settings=Signal::stored(settings)
                on_select=Callback::new(move |id: String| {
                    edit.used(&id);
                    active.set(id);
                })
                on_change=on_change
                on_save=Callback::new(move |_: (String, SaveRequest)| on_review.run(()))
                on_add=on_add
                on_close=Callback::new(move |r: CloseRequest| on_close.run((r.id, r.dirty)))
                on_dirty_change=on_dirty
                on_edit_as_change=Callback::new(move |(id, l): (String, Language)| edit.shown_as(&id, &label_of(l)))
                on_sync_change=Callback::new(move |(id, ok): (String, bool)| {
                    let now = (!ok).then(|| tabs.editor(&id).and_then(|e| e.text())).flatten();
                    edit.synced(&id, ok, now);
                })
                on_problems=Callback::new(move |(id, found): (String, Vec<Diagnostic>)| edit.editor_errors(&id, first_error(&id, &found)))
            />
        </div>
    }
    .into_any())
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::text_edit::EditFile;

    fn names(t: &EditText, new: &[String]) -> Vec<String> {
        t.files
            .iter()
            .map(|f| f.name.clone())
            .chain(new.iter().cloned())
            .collect()
    }

    fn file(name: &str, text: &str) -> EditFile {
        EditFile {
            name: name.into(),
            text: text.into(),
            read_only: None,
        }
    }

    #[test]
    fn the_editors_own_errors_name_the_file_and_line() {
        let mut d = Diagnostic::default();
        d.line = Some(3);
        d.message = "expected a value".into();
        d.severity = Severity::Error;
        assert_eq!(
            first_error("server.d/x.yaml", &[d.clone()]).as_deref(),
            Some("server.d/x.yaml has an error on line 3: expected a value")
        );
        let mut warning = d.clone();
        warning.severity = Severity::Warning;
        assert_eq!(
            first_error("server.yaml", &[warning]),
            None,
            "a warning holds nothing"
        );
        let mut host = d;
        host.origin = DiagnosticOrigin::Host;
        assert_eq!(
            first_error("server.yaml", &[host]),
            None,
            "the server's own are not the editor's"
        );
    }

    #[test]
    fn held_edit_as_text_opens_again_where_the_user_left_it() {
        let t = EditText {
            files: vec![
                file("server.yaml", "a: 1\n"),
                EditFile {
                    read_only: Some("not writable by burrow".into()),
                    ..file("server.d/ro.yaml", "c: 1\n")
                },
            ],
            ..EditText::default()
        };
        let mut held = BTreeMap::new();
        held.insert(
            "server.yaml".to_string(),
            Unwritten {
                name: "server.yaml".into(),
                shown: "TOML".into(),
                own: "YAML".into(),
                text: "a = [\n".into(),
            },
        );
        let docs = documents(
            &t,
            &BTreeMap::new(),
            &[],
            &Schemas::default(),
            &SecretOptions::default(),
            Kept {
                asked: &BTreeMap::new(),
                held: &held,
            },
            &names(&t, &[]),
        );
        assert_eq!(docs[0].edit_as, Some(Language::TOML));
        assert_eq!(docs[0].edit_as_text.as_deref(), Some("a = [\n"));
        assert_eq!(docs[0].value, "a: 1\n", "the value stays the file's");
        assert!(docs[1].read_only);
        assert_eq!(
            docs[1].read_only_reason.as_deref(),
            Some("not writable by burrow")
        );
    }

    #[test]
    fn a_drop_in_tab_checks_against_the_drop_in_schema() {
        let t = EditText {
            files: vec![
                file("server.yaml", "a: 1\n"),
                file("server.d/x.yaml", "b: 1\n"),
            ],
            schema: burrow_admin_proto::serde_json::json!({"title": "main"}),
            drop_in_schema: Some(burrow_admin_proto::serde_json::json!({"title": "drop-in"})),
            ..EditText::default()
        };
        let docs = documents(
            &t,
            &BTreeMap::new(),
            &[],
            &Schemas::of(&t),
            &SecretOptions::default(),
            Kept {
                asked: &BTreeMap::new(),
                held: &BTreeMap::new(),
            },
            &names(&t, &[]),
        );
        assert_eq!(docs.len(), 2);
        let s = Schemas::of(&t);
        assert!(std::ptr::eq(
            s.for_file(true).unwrap(),
            s.main.as_ref().unwrap()
        ));
        assert!(std::ptr::eq(
            s.for_file(false).unwrap(),
            s.drop_in.as_ref().unwrap()
        ));
        assert!(docs[0].schema.is_some() && docs[1].schema.is_some());
        let older = EditText {
            drop_in_schema: None,
            ..t.clone()
        };
        let s = Schemas::of(&older);
        assert!(
            std::ptr::eq(s.for_file(false).unwrap(), s.main.as_ref().unwrap()),
            "without a drop-in schema a drop-in uses the main one"
        );
    }

    #[test]
    fn only_another_language_is_asked_of_the_editor() {
        assert!(requested("YAML", None).is_none());
        assert!(requested("YAML", Some("YAML")).is_none());
        assert_eq!(requested("YAML", Some("TOML")), Some(Language::TOML));
        assert_eq!(requested("JSON", Some("YAML")), Some(Language::YAML));
        assert!(requested("YAML", Some("XML")).is_none());
        for l in super::super::edit_as::LANGUAGES {
            assert_eq!(language(l).map(|x| x.label()), Some(l), "{l} is built in");
        }
    }

    #[test]
    fn the_editor_comes_back_on_the_file_last_shown() {
        let files = vec!["server.yaml".to_string(), "server.d/x.yaml".to_string()];
        assert_eq!(first_shown(&files, "server.d/x.yaml"), "server.d/x.yaml");
        assert_eq!(first_shown(&files, "server.d/gone.yaml"), "server.yaml");
        assert_eq!(first_shown(&files, ""), "server.yaml");
        assert_eq!(first_shown(&[], "server.yaml"), "");
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
        let docs = documents(
            &t,
            &values,
            &[],
            &Schemas::default(),
            &SecretOptions::default(),
            Kept {
                asked: &BTreeMap::new(),
                held: &BTreeMap::new(),
            },
            &names(&t, &[]),
        );
        assert_eq!(docs.len(), 2);
        assert_eq!(
            (docs[0].id.as_str(), docs[0].value.as_str()),
            ("server.yaml", "a: 2\n")
        );
        assert_eq!(docs[0].baseline.as_deref(), Some("a: 1\n"));
        assert!(!docs[0].read_only && docs[1].read_only);
        assert_eq!(docs[1].value, "c: 1\n");
        assert!(
            docs[0].edit_as.is_none(),
            "a file is shown in its own language"
        );
        let mut asked = BTreeMap::new();
        asked.insert("server.yaml".to_string(), "TOML".to_string());
        asked.insert("server.d/ro.yaml".to_string(), "YAML".to_string());
        let docs = documents(
            &t,
            &values,
            &[],
            &Schemas::default(),
            &SecretOptions::default(),
            Kept {
                asked: &asked,
                held: &BTreeMap::new(),
            },
            &names(&t, &[]),
        );
        assert_eq!(docs[0].edit_as, Some(Language::TOML));
        assert!(
            docs[1].edit_as.is_none(),
            "asking for its own language asks nothing"
        );
        assert_eq!(
            docs[0].value, "a: 2\n",
            "the value stays in the file's language"
        );
        assert!(!docs[0].closable, "the main file is never closed");
        assert!(docs[1].closable, "a drop-in tab closes; the file stays");
        let mut values = values.clone();
        values.insert(
            "server.d/new.yaml".to_string(),
            "services: {}\n".to_string(),
        );
        let docs = documents(
            &t,
            &values,
            &[],
            &Schemas::default(),
            &SecretOptions::default(),
            Kept {
                asked: &BTreeMap::new(),
                held: &BTreeMap::new(),
            },
            &names(&t, &["server.d/new.yaml".to_string()]),
        );
        assert_eq!(docs.len(), 3);
        assert_eq!(docs[2].id, "server.d/new.yaml");
        assert!(docs[2].closable, "a new file can be closed");
        assert_eq!(docs[2].baseline.as_deref(), Some(""));
        assert_eq!(docs[2].value, "services: {}\n");
        let only = documents(
            &t,
            &values,
            &[],
            &Schemas::default(),
            &SecretOptions::default(),
            Kept {
                asked: &BTreeMap::new(),
                held: &BTreeMap::new(),
            },
            &["server.d/ro.yaml".to_string(), "server.yaml".to_string()],
        );
        assert_eq!(
            only.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            ["server.d/ro.yaml", "server.yaml"],
            "only open tabs are documents, in tab order"
        );
    }
}
