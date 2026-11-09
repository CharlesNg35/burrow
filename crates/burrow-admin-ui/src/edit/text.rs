use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use burrow_admin_proto::Path;
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_admin_proto::msg::{self, ErrorBody, ErrorCode};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::Plan;
use burrow_admin_proto::serde::Deserialize;
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_admin_proto::text_edit::{EditFile, EditText, TextFile, placeholder};
use leptos::prelude::*;

use crate::api;

pub const TEXT_CHECK_AFTER: Duration = Duration::from_millis(700);

pub type Hook = std::rc::Rc<dyn Fn() -> Option<super::leave::Unwritten>>;

type Banner = (RwSignal<Option<msg::ConfigEvent>>, Signal<bool>);

pub type LoadedText = Option<Result<Arc<EditText>, String>>;

#[derive(Clone, Copy)]
pub struct TextEdit {
    pub open: RwSignal<bool>,
    pub loaded: RwSignal<LoadedText>,
    pub values: RwSignal<BTreeMap<String, String>>,
    pub problems: RwSignal<Vec<Diag>>,
    pub unsavable: RwSignal<Option<String>>,
    pub active: RwSignal<String>,
    pub before_review: StoredValue<Option<Hook>, LocalStorage>,
    pub edit_as: RwSignal<BTreeMap<String, String>>,
    pub shown: RwSignal<BTreeMap<String, String>>,
    pub new_files: RwSignal<Vec<String>>,
    pub adding: RwSignal<bool>,
    pub closing: RwSignal<Option<String>>,
    pub tabs: RwSignal<Vec<String>>,
    pub recent: RwSignal<Vec<String>>,
    pub unsaved: RwSignal<BTreeSet<String>>,
    pub spot: RwSignal<Option<(String, super::tabs::Spot)>>,
    pub leaving: RwSignal<Option<super::leave::Unwritten>>,
    pub lost: RwSignal<Option<String>>,
    banner: StoredValue<Option<Banner>>,
    pub(super) then: StoredValue<Option<super::leave::Then>, LocalStorage>,
    pub(super) pending_pop: StoredValue<Option<String>>,
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

const OPEN: &str = "«secret ";
const CLOSE: &str = "»";

fn tokens(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find(OPEN) {
        let start = at + i;
        let id_at = start + OPEN.len();
        let id = text.get(id_at..id_at + 8).unwrap_or("");
        let is_id = id.len() == 8 && id.bytes().all(|b| b.is_ascii_hexdigit());
        if is_id && text[id_at + 8..].starts_with(CLOSE) {
            let end = id_at + 8 + CLOSE.len();
            out.push((start..end, id));
            at = end;
        } else {
            at = id_at;
        }
    }
    out
}

fn skeleton(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (r, _) in tokens(text) {
        out.push_str(&text[at..r.start]);
        out.push_str(OPEN);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}

fn new_ids(was: &str, now: &str) -> Option<BTreeMap<String, String>> {
    let (a, b) = (tokens(was), tokens(now));
    if a.len() != b.len() || skeleton(was) != skeleton(now) {
        return None;
    }
    let mut map = BTreeMap::new();
    for ((_, from), (_, to)) in a.iter().zip(&b) {
        if map
            .insert(from.to_string(), to.to_string())
            .is_some_and(|t| t != *to)
        {
            return None;
        }
    }
    Some(map)
}

fn with_ids(typed: &str, map: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(typed.len());
    let mut at = 0;
    for (r, id) in tokens(typed) {
        out.push_str(&typed[at..r.start]);
        match map.get(id) {
            Some(to) => out.push_str(&placeholder(to)),
            None => out.push_str(&typed[r.clone()]),
        }
        at = r.end;
    }
    out.push_str(&typed[at..]);
    out
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
            let typed = values.get(&f.name).and_then(|v| {
                let w = was(&f.name).filter(|w| *w != v.as_str())?;
                Some(match new_ids(w, &f.text) {
                    Some(map) => with_ids(v, &map),
                    None => v.clone(),
                })
            });
            (f.name.clone(), typed.unwrap_or_else(|| f.text.clone()))
        })
        .collect()
}

pub fn settle_banner(changed: RwSignal<Option<msg::ConfigEvent>>, draft_dirty: bool) {
    if !draft_dirty && changed.with_untracked(Option::is_some) {
        changed.set(None);
    }
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

pub fn refusal(err: &ErrorBody) -> Vec<Diag> {
    if let Ok(body) = serde_json::from_value::<msg::InvalidBody>(err.body.clone())
        && !body.diags.is_empty()
    {
        return body.diags;
    }
    let at: At = serde_json::from_value(err.body.clone()).unwrap_or_default();
    vec![Diag {
        severity: Severity::Error,
        code: err.code.as_str().to_string(),
        message: err.message.clone(),
        path: at.path,
        also: Vec::new(),
        label: None,
        help: None,
        line: at.line,
        col: at.col,
        end_line: None,
        end_col: None,
        before: false,
        subjects: Vec::new(),
        file: at.file,
    }]
}

impl TextEdit {
    pub fn new() -> TextEdit {
        TextEdit {
            open: RwSignal::new(false),
            loaded: RwSignal::new(None),
            values: RwSignal::new(BTreeMap::new()),
            problems: RwSignal::new(Vec::new()),
            unsavable: RwSignal::new(None),
            active: RwSignal::new(String::new()),
            before_review: StoredValue::new_local(None),
            edit_as: RwSignal::new(BTreeMap::new()),
            shown: RwSignal::new(BTreeMap::new()),
            new_files: RwSignal::new(Vec::new()),
            adding: RwSignal::new(false),
            closing: RwSignal::new(None),
            tabs: RwSignal::new(Vec::new()),
            recent: RwSignal::new(Vec::new()),
            unsaved: RwSignal::new(BTreeSet::new()),
            spot: RwSignal::new(None),
            leaving: RwSignal::new(None),
            lost: RwSignal::new(None),
            banner: StoredValue::new(None),
            then: StoredValue::new_local(None),
            pending_pop: StoredValue::new(None),
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
        self.active.set(String::new());
        self.edit_as.set(BTreeMap::new());
        self.shown.set(BTreeMap::new());
        self.new_files.set(Vec::new());
        self.adding.set(false);
        self.closing.set(None);
        self.tabs.set(Vec::new());
        self.unsaved.set(BTreeSet::new());
        self.spot.set(None);
        self.leaving.set(None);
        self.lost.set(None);
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
        let (mut values, gone) = self.loaded.with_untracked(|l| {
            let old = l.as_ref().and_then(|r| r.as_ref().ok()).map(|a| a.as_ref());
            self.values.with_untracked(|v| {
                let gone = self.unsaved.with_untracked(|u| {
                    self.new_files
                        .with_untracked(|n| super::gone::gone_edits(old, &next, v, u, n))
                });
                (rebased(old, &next, v), gone)
            })
        });
        let still = self.new_files.with_untracked(|n| {
            self.values
                .with_untracked(|typed| super::new_file::kept_new(&next, n, typed, &mut values))
        });
        self.prune(&next, &still);
        if let Some(note) = super::gone::gone_note(&gone) {
            self.lost.set(Some(note));
        }
        if self.new_files.with_untracked(|n| *n != still) {
            self.new_files.set(still);
        }
        let main = next
            .files
            .first()
            .map(|f| f.name.clone())
            .unwrap_or_default();
        let tabs = self.tabs.with_untracked(|tabs| {
            self.new_files.with_untracked(|n| {
                super::tabs::kept_tabs(tabs, &main, |t| {
                    next.files.iter().any(|f| f.name == t) || n.iter().any(|x| x == t)
                })
            })
        });
        if self.tabs.with_untracked(|t| *t != tabs) {
            self.tabs.set(tabs);
        }
        self.values.set(values);
        self.loaded.set(Some(Ok(Arc::new(next))));
        self.settle();
        self.changed();
    }

    pub fn watch_banner(
        &self,
        changed: RwSignal<Option<msg::ConfigEvent>>,
        draft_dirty: Signal<bool>,
    ) {
        self.banner.set_value(Some((changed, draft_dirty)));
    }

    pub fn settle(&self) {
        if let Some((changed, draft_dirty)) = self.banner.get_value() {
            settle_banner(changed, untrack(|| draft_dirty.get()));
        }
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
            Some(Ok(t)) => self.values.with(|v| {
                self.new_files
                    .with(|n| super::new_file::with_new(changed_of(&t.files, v), n, v))
            }),
            _ => Vec::new(),
        })
    }

    pub fn dirty(&self) -> bool {
        !self.changed_files().is_empty()
            || self.new_files.with(|n| !n.is_empty())
            || self.unsaved.with(|u| !u.is_empty())
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
                Err(e) => me.problems.set(refusal(&e)),
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
    fn a_text_review_stands_alone_and_comes_back_to_the_config_file_tab() {
        use super::super::Source;
        assert!(Source::Text.standalone() && Source::Revert("b2s:1".into()).standalone());
        assert!(!Source::Draft.standalone());
        assert_eq!(Source::Text.revert_of(), None);
        assert_eq!(
            Source::Revert("b2s:1".into()).revert_of().as_deref(),
            Some("b2s:1")
        );
        assert_eq!(
            crate::route::file_tab(),
            (crate::route::Route::Settings, Some(("tab", "file")))
        );
    }

    #[test]
    fn typed_text_takes_the_new_placeholder_ids_when_only_the_ids_changed() {
        let was = "a: «secret 0000000a»\nb: «secret 0000000b»\nc: 1\n";
        let old = edit("b2s:1", vec![file("server.yaml", was, false)]);
        let next = edit(
            "b2s:1",
            vec![file(
                "server.yaml",
                "a: «secret 1111111a»\nb: «secret 1111111b»\nc: 1\n",
                false,
            )],
        );
        let values = typed(&[(
            "server.yaml",
            "b: «secret 0000000a»\nc: 2\nd: «secret 0000000c»\n",
        )]);
        assert_eq!(
            rebased(Some(&old), &next, &values)["server.yaml"],
            "b: «secret 1111111a»\nc: 2\nd: «secret 0000000c»\n",
            "ids from the old text follow, a typed id the server never gave stays"
        );
        let other = edit(
            "b2s:2",
            vec![file(
                "server.yaml",
                "a: «secret 1111111a»\nb: «secret 1111111b»\nc: 5\n",
                false,
            )],
        );
        assert_eq!(
            rebased(Some(&old), &other, &values)["server.yaml"],
            values["server.yaml"],
            "a text that changed beyond its ids is left to the editor's notice"
        );
    }

    #[test]
    fn a_rebase_outside_any_component_takes_down_the_changed_banner() {
        let ev = msg::ConfigEvent {
            rev: "b2s:2".into(),
            by: "ops".into(),
            ok: true,
        };
        let changed = RwSignal::new(Some(ev.clone()));
        let draft_dirty = RwSignal::new(true);
        let t = TextEdit::new();
        t.watch_banner(changed, draft_dirty.into());
        assert!(
            leptos::prelude::Owner::current().is_none(),
            "as in a reply callback: no component around"
        );
        t.settle();
        assert_eq!(
            changed.get_untracked(),
            Some(ev),
            "the draft still needs it"
        );
        draft_dirty.set(false);
        t.settle();
        assert_eq!(changed.get_untracked(), None);
    }

    #[test]
    fn a_rebased_text_takes_down_the_changed_banner_unless_the_draft_has_changes() {
        let ev = msg::ConfigEvent {
            rev: "b2s:2".into(),
            by: "ops".into(),
            ok: true,
        };
        let changed = RwSignal::new(Some(ev.clone()));
        settle_banner(changed, true);
        assert_eq!(
            changed.get_untracked(),
            Some(ev),
            "the draft still needs it"
        );
        settle_banner(changed, false);
        assert_eq!(changed.get_untracked(), None);
    }

    #[test]
    fn a_refused_check_becomes_a_problem_where_the_server_says() {
        let err = ErrorBody {
            code: ErrorCode::Invalid,
            message: "this placeholder stands for the secret of clients.home.secret".into(),
            body: json!({"file": "server.d/10-web.yaml", "line": 4, "col": 9, "path": ["visitors", "lap", "secret"]}),
        };
        let d = refusal(&err).remove(0);
        assert!(d.is_error());
        assert_eq!(d.code, "invalid");
        assert_eq!(d.file.as_deref(), Some("server.d/10-web.yaml"));
        assert_eq!((d.line, d.col), (Some(4), Some(9)));
        assert_eq!(d.path.unwrap().join("."), "visitors.lap.secret");
        let bare = refusal(&ErrorBody {
            code: ErrorCode::NotWritable,
            message: "x".into(),
            body: Value::Null,
        })
        .remove(0);
        assert!(bare.file.is_none() && bare.path.is_none() && bare.line.is_none());
        let placeholders = ErrorBody {
            code: ErrorCode::Invalid,
            message: "placeholders".into(),
            body: json!({"diags": [
                {"severity": "error", "code": "secret-placeholder", "message": "server.yaml line 11, visitors.ops.secret: this placeholder stands for the secret of clients.home.secret", "path": ["visitors", "ops", "secret"], "line": 11, "col": 13, "end_line": 11, "end_col": 30},
                {"severity": "error", "code": "secret-placeholder", "message": "«secret 0badc0de» stands for no current secret", "path": ["clients", "box", "secret"], "file": "server.d/tool.yaml", "line": 3}
            ]}),
        };
        let got = refusal(&placeholders);
        assert_eq!(got.len(), 2, "one problem per placeholder");
        assert_eq!(got[0].code, burrow_admin_proto::codes::SECRET_PLACEHOLDER);
        assert_eq!((got[0].end_line, got[0].end_col), (Some(11), Some(30)));
        assert_eq!(got[1].file.as_deref(), Some("server.d/tool.yaml"));
    }
}
