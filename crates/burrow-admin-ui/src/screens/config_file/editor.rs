use std::collections::BTreeMap;
use std::sync::Arc;

use burrow_admin_proto::diag::Diag;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::text_edit::{EditFile, EditText, PLACEHOLDER_PATTERN};
use config_editor_leptos::{
    CloseRequest, ConfigDocuments, Document, DocumentsHandle, EditorSettings, EditorTheme,
    Language, SaveRequest, Schema, SchemaSource, SecretOptions, Severity, check_syntax,
    flush_pending_edits,
};
use leptos::prelude::*;

use crate::edit::{Spot, Unwritten};

use super::edit_as::{own_label, seen};
use super::follow::follow;
use super::problems::host_problems;

fn undo_key(e: &web_sys::KeyboardEvent) -> bool {
    (e.ctrl_key() || e.meta_key()) && matches!(e.key().as_str(), "z" | "Z" | "y" | "Y")
}

const HELD_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

pub const SECRET_HINT: &str = "The current secret, kept as it is. Type a new value to replace it.";

#[derive(Clone, Copy)]
pub struct EditorProps {
    pub text: Signal<Option<Arc<EditText>>>,
    pub values: Signal<BTreeMap<String, String>>,
    pub problems: Signal<Vec<Diag>>,
    pub on_change: Callback<(String, String)>,
    pub on_review: Callback<()>,
    pub unsavable: RwSignal<Option<String>>,
    pub before_review: StoredValue<Option<crate::edit::Hook>, LocalStorage>,
    pub active: RwSignal<String>,
    pub edit_as: RwSignal<BTreeMap<String, String>>,
    pub shown: RwSignal<BTreeMap<String, String>>,
    pub new_files: Signal<Vec<String>>,
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

fn documents(
    t: &EditText,
    values: &BTreeMap<String, String>,
    problems: &[Diag],
    schemas: &Schemas,
    secrets: &SecretOptions,
    asked: &BTreeMap<String, String>,
    open: &[String],
) -> Vec<Document> {
    let main = t.files.first().map(|f| f.name.as_str()).unwrap_or_default();
    open.iter()
        .filter_map(|name| {
            let d = match t.files.iter().find(|f| f.name == *name) {
                Some(f) => {
                    let now = values.get(&f.name).unwrap_or(&f.text).clone();
                    Document::new(f.name.clone(), f.name.clone(), now)
                        .baseline(f.text.clone())
                        .read_only(f.read_only.is_some())
                        .closable(f.name != main)
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
            let d = match requested(own_label(name), asked.get(name).map(String::as_str)) {
                Some(l) => d.edit_as(l),
                None => d,
            };
            Some(match schemas.for_file(name == main) {
                Some(s) => d.schema(s.clone()),
                None => d,
            })
        })
        .collect()
}

fn names(t: &EditText, new: &[String]) -> Vec<String> {
    t.files
        .iter()
        .map(|f| f.name.clone())
        .chain(new.iter().cloned())
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

fn unwritten(files: &[String], switch: Shown) -> Vec<Unwritten> {
    files
        .iter()
        .filter_map(|name| {
            let editor = switch.tabs.editor(name).filter(|e| e.is_mounted())?;
            let text = editor.text().unwrap_or_default();
            let host = editor.value();
            let shown = switch.apply(name, Some(&text), host.as_deref());
            if host.is_some() {
                return None;
            }
            let own = own_label(name);
            Some(Unwritten {
                name: name.clone(),
                shown: if shown == own { "edited".into() } else { shown },
                own: own.to_string(),
                text,
            })
        })
        .collect()
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

#[derive(Clone, Copy)]
struct Shown {
    asked: RwSignal<BTreeMap<String, String>>,
    shown: RwSignal<BTreeMap<String, String>>,
    tabs: DocumentsHandle,
    edit: crate::edit::TextEdit,
}

impl Shown {
    fn apply(&self, name: &str, on_screen: Option<&str>, host: Option<&str>) -> String {
        let own = own_label(name);
        let asked = self
            .asked
            .with_untracked(|a| a.get(name).cloned())
            .unwrap_or_else(|| own.to_string());
        let before = self.shown.with_untracked(|s| s.get(name).cloned());
        let picked = self.edit.picked.with_value(|p| p.contains(name));
        let now = seen(own, &asked, before.as_deref(), picked, on_screen, host);
        if now.settled && picked {
            self.edit.picked.update_value(|p| {
                p.remove(name);
            });
        }
        if now.asked_back {
            self.asked.update(|a| {
                a.insert(name.to_string(), own.to_string());
            });
        }
        let waiting_pick = picked && !now.settled;
        if waiting_pick || before.as_deref() != Some(now.shown.as_str()) {
            self.shown.update(|s| {
                s.insert(name.to_string(), now.shown.clone());
            });
        }
        now.shown
    }

    fn refresh(&self, name: &str) {
        let editor = self.tabs.editor(name).filter(|e| e.is_mounted());
        let on_screen = editor.and_then(|e| e.text());
        let host = editor.and_then(|e| e.value());
        self.apply(name, on_screen.as_deref(), host.as_deref());
    }

    fn refresh_soon(&self, name: String) {
        let me = *self;
        for ms in [60, 400, 1200] {
            let name = name.clone();
            set_timeout(
                move || me.refresh(&name),
                std::time::Duration::from_millis(ms),
            );
        }
    }
}

#[leptos::wasm_split::wasm_split(config_editor, wasm_split_path = ::leptos::wasm_split, fallible)]
pub fn text_editor(p: EditorProps) -> Result<AnyView, burrow_ui_kit::LoadFailed> {
    let EditorProps {
        text,
        values,
        problems,
        on_change,
        on_review,
        unsavable,
        before_review,
        active,
        edit_as,
        shown,
        new_files,
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
    let list = Signal::derive(move || {
        text.with(|t| {
            t.as_ref().map_or_else(Vec::new, |t| {
                values.with(|v| {
                    problems.with(|ps| {
                        edit_as
                            .with(|a| open.with(|o| documents(t, v, ps, &schemas, &secrets, a, o)))
                    })
                })
            })
        })
    });
    Effect::new(move |_| {
        let why = text.with(|t| {
            t.as_ref().and_then(|t| {
                let mut files = t.files.clone();
                new_files.with(|n| {
                    files.extend(n.iter().map(|name| EditFile {
                        name: name.clone(),
                        ..EditFile::default()
                    }))
                });
                values.with(|v| syntax_problem(&files, v))
            })
        });
        if unsavable.with_untracked(|u| *u != why) {
            unsavable.set(why);
        }
    });
    let settings = EditorSettings {
        theme: EditorTheme::ViraInspired,
        show_footer: true,
        styled: true,
        ..EditorSettings::default()
    };
    let tabs = DocumentsHandle::new();
    let switch = Shown {
        asked: edit_as,
        shown,
        tabs,
        edit,
    };
    Effect::new(move |_| {
        edit_as.track();
        switch.refresh_soon(active.get_untracked());
    });
    let typed = StoredValue::new(0u64);
    let check_held = move || {
        typed.update_value(|n| *n += 1);
        let n = typed.get_value();
        set_timeout(
            move || {
                if typed.try_get_value() == Some(n) {
                    edit.note_unwritten();
                }
            },
            HELD_AFTER,
        );
    };
    before_review.set_value(Some(std::rc::Rc::new(move || {
        flush_pending_edits();
        let files = text.with_untracked(|t| {
            t.as_ref()
                .map(|t| names(t, &new_files.get_untracked()))
                .unwrap_or_default()
        });
        unwritten(&files, switch)
    })));
    on_cleanup(move || {
        let _ = before_review.try_set_value(None);
    });
    follow(active, spot, tabs);
    for name in edit_as.with_untracked(|a| a.keys().cloned().collect::<Vec<_>>()) {
        switch.refresh_soon(name);
    }
    Ok(view! {
        <div
            class="min-w-0 w-full"
            data-testid="file-editor"
            on:input=move |_| check_held()
            on:beforeinput=move |_| check_held()
            on:keydown=move |e| {
                if undo_key(&e) {
                    let name = active.get_untracked();
                    edit.picked.update_value(|p| {
                        p.remove(&name);
                    });
                }
                check_held();
            }
            on:paste=move |_| check_held()
        >
            <ConfigDocuments
                documents=list
                initial_active=main
                handle=tabs
                label="Config files"
                settings=Signal::stored(settings)
                on_select=Callback::new(move |id: String| {
                    edit.used(&id);
                    active.set(id.clone());
                    switch.refresh_soon(id);
                })
                on_change=Callback::new(move |change: (String, String)| {
                    check_held();
                    switch.refresh(&change.0);
                    on_change.run(change);
                })
                on_save=Callback::new(move |_: (String, SaveRequest)| on_review.run(()))
                on_add=on_add
                on_close=Callback::new(move |r: CloseRequest| on_close.run((r.id, r.dirty)))
                on_dirty_change=Callback::new(move |d: (String, bool)| {
                    check_held();
                    on_dirty.run(d);
                })
            />
        </div>
    }
    .into_any())
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
            &BTreeMap::new(),
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
            &BTreeMap::new(),
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
            &asked,
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
            &BTreeMap::new(),
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
            &BTreeMap::new(),
            &["server.d/ro.yaml".to_string(), "server.yaml".to_string()],
        );
        assert_eq!(
            only.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            ["server.d/ro.yaml", "server.yaml"],
            "only open tabs are documents, in tab order"
        );
    }
}
