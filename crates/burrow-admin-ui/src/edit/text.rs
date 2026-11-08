use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use burrow_admin_proto::Path;
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_admin_proto::msg::{self, ErrorBody, ErrorCode};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::Plan;
use burrow_admin_proto::serde::Deserialize;
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_admin_proto::text_edit::{EditFile, EditText, TextFile};
use leptos::prelude::*;

use crate::api;

pub const TEXT_CHECK_AFTER: Duration = Duration::from_millis(700);

pub type LoadedText = Option<Result<Arc<EditText>, String>>;

#[derive(Clone, Copy)]
pub struct TextEdit {
    pub open: RwSignal<bool>,
    pub loaded: RwSignal<LoadedText>,
    pub values: RwSignal<BTreeMap<String, String>>,
    pub problems: RwSignal<Vec<Diag>>,
    pub unsavable: RwSignal<Option<String>>,
    pub flush: StoredValue<Option<Callback<()>>>,
    checks: StoredValue<u64>,
    fetches: StoredValue<u64>,
}

pub fn changed_of(files: &[EditFile], values: &BTreeMap<String, String>) -> Vec<TextFile> {
    files
        .iter()
        .filter(|f| f.read_only.is_none())
        .filter_map(|f| {
            let now = values.get(&f.name)?;
            (*now != f.text).then(|| TextFile {
                name: f.name.clone(),
                text: now.clone(),
            })
        })
        .collect()
}

pub fn rebased(
    old: Option<&EditText>,
    next: &EditText,
    values: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let was = |name: &str| {
        old.and_then(|o| o.files.iter().find(|f| f.name == name))
            .map(|f| f.text.as_str())
    };
    next.files
        .iter()
        .map(|f| {
            let typed = values
                .get(&f.name)
                .filter(|v| was(&f.name).is_some_and(|w| w != v.as_str()));
            (f.name.clone(), typed.unwrap_or(&f.text).clone())
        })
        .collect()
}

pub fn text_ops(files: Vec<TextFile>) -> Vec<Op> {
    if files.is_empty() {
        Vec::new()
    } else {
        vec![Op::ReplaceText { files }]
    }
}

#[derive(Default, Deserialize)]
#[serde(crate = "burrow_admin_proto::serde", default)]
struct At {
    file: Option<String>,
    line: Option<u32>,
    col: Option<u32>,
    path: Option<Path>,
}

pub fn refusal(err: &ErrorBody) -> Diag {
    let at: At = serde_json::from_value(err.body.clone()).unwrap_or_default();
    Diag {
        severity: Severity::Error,
        code: err.code.as_str().to_string(),
        message: err.message.clone(),
        path: at.path,
        also: Vec::new(),
        label: None,
        help: None,
        line: at.line,
        col: at.col,
        before: false,
        subjects: Vec::new(),
        file: at.file,
    }
}

impl TextEdit {
    pub fn new() -> TextEdit {
        TextEdit {
            open: RwSignal::new(false),
            loaded: RwSignal::new(None),
            values: RwSignal::new(BTreeMap::new()),
            problems: RwSignal::new(Vec::new()),
            unsavable: RwSignal::new(None),
            flush: StoredValue::new(None),
            checks: StoredValue::new(0),
            fetches: StoredValue::new(0),
        }
    }

    pub fn start(&self) {
        self.close();
        self.open.set(true);
        self.fetch();
    }

    pub fn close(&self) {
        self.checks.update_value(|n| *n += 1);
        self.fetches.update_value(|n| *n += 1);
        self.open.set(false);
        self.loaded.set(None);
        self.values.set(BTreeMap::new());
        self.problems.set(Vec::new());
        self.unsavable.set(None);
    }

    pub fn fetch(&self) {
        self.fetches.update_value(|n| *n += 1);
        let asked = self.fetches.get_value();
        let me = *self;
        api::request(msg::req::CONFIG_TEXT, json!({}), move |r| {
            if me.fetches.try_get_value() != Some(asked) {
                return;
            }
            match r
                .map_err(|e| e.message)
                .and_then(|v| serde_json::from_value::<EditText>(v).map_err(|e| e.to_string()))
            {
                Ok(next) => me.take(next),
                Err(e) => {
                    if me.loaded.with_untracked(|l| !matches!(l, Some(Ok(_)))) {
                        me.loaded.set(Some(Err(e)));
                    }
                }
            }
        });
    }

    pub fn take(&self, next: EditText) {
        let values = self.loaded.with_untracked(|l| {
            let old = l.as_ref().and_then(|r| r.as_ref().ok()).map(|a| a.as_ref());
            self.values.with_untracked(|v| rebased(old, &next, v))
        });
        self.values.set(values);
        self.loaded.set(Some(Ok(Arc::new(next))));
        self.changed();
    }

    pub fn text(&self) -> Option<Arc<EditText>> {
        self.loaded
            .with(|l| l.as_ref().and_then(|r| r.as_ref().ok()).cloned())
    }

    pub fn base(&self) -> String {
        self.loaded.with_untracked(|l| match l {
            Some(Ok(t)) => t.rev.clone(),
            _ => String::new(),
        })
    }

    pub fn changed_files(&self) -> Vec<TextFile> {
        self.loaded.with(|l| match l {
            Some(Ok(t)) => self.values.with(|v| changed_of(&t.files, v)),
            _ => Vec::new(),
        })
    }

    pub fn dirty(&self) -> bool {
        !self.changed_files().is_empty()
    }

    pub fn ops(&self) -> Vec<Op> {
        text_ops(untrack(|| self.changed_files()))
    }

    pub fn set_value(&self, name: &str, text: String) {
        if self.values.with_untracked(|v| v.get(name) == Some(&text)) {
            return;
        }
        self.values.update(|v| {
            v.insert(name.to_string(), text);
        });
        self.changed();
    }

    fn changed(&self) {
        self.checks.update_value(|n| *n += 1);
        let n = self.checks.get_value();
        let me = *self;
        set_timeout(move || me.check(n), TEXT_CHECK_AFTER);
    }

    fn check(&self, n: u64) {
        if self.checks.try_get_value() != Some(n) {
            return;
        }
        let ops = self.ops();
        if ops.is_empty() {
            if self.problems.with_untracked(|p| !p.is_empty()) {
                self.problems.set(Vec::new());
            }
            return;
        }
        let me = *self;
        let body = api::body(&msg::Validate {
            base: self.base(),
            ops,
        });
        api::request_within(msg::req::VALIDATE, body, api::CHECK_WITHIN, move |r| {
            if me.checks.try_get_value() != Some(n) {
                return;
            }
            match r {
                Ok(v) => {
                    if let Ok(p) = serde_json::from_value::<Plan>(v) {
                        me.problems.set(p.diags);
                    }
                }
                Err(e) if e.code == ErrorCode::Conflict => me.fetch(),
                Err(e) if e.body.get("unanswered") == Some(&Value::Bool(true)) => {}
                Err(e) => me.problems.set(vec![refusal(&e)]),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, text: &str, ro: bool) -> EditFile {
        EditFile {
            name: name.into(),
            text: text.into(),
            read_only: ro.then(|| "not writable by burrow".into()),
        }
    }

    fn edit(rev: &str, files: Vec<EditFile>) -> EditText {
        EditText {
            rev: rev.into(),
            files,
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
    fn only_edited_writable_files_are_sent_in_one_replace_text() {
        let files = vec![
            file("server.yaml", "a: 1\n", false),
            file("server.d/10-web.yaml", "b: 1\n", false),
            file("server.d/20-ro.yaml", "c: 1\n", true),
        ];
        let values = typed(&[
            ("server.yaml", "a: 1\n"),
            ("server.d/10-web.yaml", "b: 2\n"),
            ("server.d/20-ro.yaml", "c: 9\n"),
        ]);
        let ops = text_ops(changed_of(&files, &values));
        assert_eq!(
            ops,
            vec![Op::ReplaceText {
                files: vec![TextFile {
                    name: "server.d/10-web.yaml".into(),
                    text: "b: 2\n".into()
                }]
            }]
        );
        assert!(text_ops(changed_of(&files, &typed(&[("server.yaml", "a: 1\n")]))).is_empty());
        assert!(text_ops(changed_of(&files, &BTreeMap::new())).is_empty());
    }

    #[test]
    fn a_new_server_text_keeps_typed_edits_and_follows_untouched_files() {
        let old = edit(
            "b2s:1",
            vec![
                file("server.yaml", "a: 1\n", false),
                file("server.d/x.yaml", "x: 1\n", false),
            ],
        );
        let next = edit(
            "b2s:2",
            vec![
                file("server.yaml", "a: 5\n", false),
                file("server.d/x.yaml", "x: 2\n", false),
                file("server.d/y.yaml", "y: 1\n", false),
            ],
        );
        let values = typed(&[
            ("server.yaml", "a: 7\n"),
            ("server.d/x.yaml", "x: 1\n"),
            ("server.d/gone.yaml", "g: 1\n"),
        ]);
        let got = rebased(Some(&old), &next, &values);
        assert_eq!(
            got,
            typed(&[
                ("server.yaml", "a: 7\n"),
                ("server.d/x.yaml", "x: 2\n"),
                ("server.d/y.yaml", "y: 1\n"),
            ])
        );
        assert_eq!(
            rebased(None, &next, &BTreeMap::new())["server.yaml"],
            "a: 5\n"
        );
    }

    #[test]
    fn a_refused_check_becomes_a_problem_where_the_server_says() {
        let err = ErrorBody {
            code: ErrorCode::Invalid,
            message: "this placeholder stands for the secret of clients.home.secret".into(),
            body: json!({"file": "server.d/10-web.yaml", "line": 4, "col": 9, "path": ["visitors", "lap", "secret"]}),
        };
        let d = refusal(&err);
        assert!(d.is_error());
        assert_eq!(d.code, "invalid");
        assert_eq!(d.file.as_deref(), Some("server.d/10-web.yaml"));
        assert_eq!((d.line, d.col), (Some(4), Some(9)));
        assert_eq!(d.path.unwrap().join("."), "visitors.lap.secret");
        let bare = refusal(&ErrorBody {
            code: ErrorCode::NotWritable,
            message: "x".into(),
            body: Value::Null,
        });
        assert!(bare.file.is_none() && bare.path.is_none() && bare.line.is_none());
    }
}
