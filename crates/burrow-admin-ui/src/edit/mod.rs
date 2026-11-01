mod check;
mod diags;
mod session;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use burrow_admin_logic::changeset::Changeset;
use burrow_admin_proto::Path;
use burrow_admin_proto::msg::{ErrorBody, ErrorCode};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::{Applied, Conflict, Plan, Refusal, ReloadFailed};
use burrow_admin_proto::serde_json::{self, Value};
use burrow_config::{Entry, ServiceEntry};
use leptos::prelude::*;

use diags::related_errors;
pub(crate) use diags::{diags_at, severity_tone};

pub const VALIDATE_AFTER: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub title: String,
    pub ops: Vec<Op>,
    pub base: String,
    pub plan: Option<Arc<Plan>>,
    pub revert_of: Option<String>,
    pub own: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Applied(Applied),
    Failed { code: ErrorCode, message: String },
    Refused(Vec<Refusal>),
    ReloadFailed(ReloadFailed),
    Unanswered,
}

#[derive(Clone, Copy)]
pub struct Edit {
    pub draft: RwSignal<Changeset>,
    pub base_rev: RwSignal<String>,
    pub texts: RwSignal<BTreeMap<Path, String>>,
    pub local: RwSignal<BTreeMap<Path, String>>,
    pub related: Memo<BTreeMap<Path, String>>,
    pub plan: RwSignal<Option<Arc<Plan>>>,
    pub checking: RwSignal<bool>,
    pub check_error: RwSignal<Option<ErrorBody>>,
    pub conflict: RwSignal<Option<Conflict>>,
    pub conflict_revert: RwSignal<Option<String>>,
    pub conflict_open: RwSignal<bool>,
    pub review: RwSignal<Option<Review>>,
    pub confirms: RwSignal<BTreeMap<String, String>>,
    pub applying: RwSignal<bool>,
    pub outcome: RwSignal<Option<Outcome>>,
    pub reset_on_load: RwSignal<bool>,
    pub auto_apply: RwSignal<bool>,
    pub refused: RwSignal<std::collections::BTreeSet<Path>>,
    pub editing: RwSignal<BTreeMap<Path, Option<Value>>>,
    pub after_apply: RwSignal<Option<crate::route::Route>>,
    seq: StoredValue<u64>,
    carry: StoredValue<Vec<(Path, Option<Value>)>>,
}

fn restore(c: &mut Changeset, path: &[String], snap: Option<&Value>) {
    let now = burrow_admin_proto::value::get(c.draft(), path).cloned();
    if now.as_ref() == snap {
        return;
    }
    match snap {
        Some(v) => c.set(path, v.clone()),
        None => c.remove(path),
    }
}

fn ops_of(draft: &Changeset, texts: &BTreeMap<Path, String>) -> Vec<Op> {
    let mut ops = draft.ops();
    for (path, text) in texts {
        ops.push(Op::SetText {
            path: path.clone(),
            text: text.clone(),
        });
    }
    ops
}

pub fn missing_confirms(plan: &Plan, typed: &BTreeMap<String, String>) -> Vec<String> {
    plan.confirms()
        .into_iter()
        .filter(|c| typed.get(*c).map(|t| t.trim()) != Some(*c))
        .map(str::to_string)
        .collect()
}

fn outcome_of(err: &ErrorBody) -> Outcome {
    if err.body.get("unanswered") == Some(&Value::Bool(true)) {
        return Outcome::Unanswered;
    }
    match err.code {
        ErrorCode::LockedOut => Outcome::Refused(
            serde_json::from_value::<burrow_admin_proto::msg::LockedOut>(err.body.clone())
                .map(|l| l.reasons)
                .unwrap_or_default(),
        ),
        ErrorCode::ReloadFailed => match serde_json::from_value::<ReloadFailed>(err.body.clone()) {
            Ok(r) => Outcome::ReloadFailed(r),
            Err(_) => Outcome::Failed {
                code: err.code,
                message: err.message.clone(),
            },
        },
        code => Outcome::Failed {
            code,
            message: err.message.clone(),
        },
    }
}

fn revert_outcome(err: &ErrorBody) -> Outcome {
    match outcome_of(err) {
        Outcome::Unanswered => Outcome::Failed {
            code: ErrorCode::Unknown,
            message: "The check of the revert did not finish because the connection closed; nothing was applied. Try again once the connection is back.".into(),
        },
        other => other,
    }
}

fn conflict_of(err: &ErrorBody) -> Option<Conflict> {
    (err.code == ErrorCode::Conflict).then(|| {
        serde_json::from_value::<Conflict>(err.body.clone()).unwrap_or(Conflict {
            current: String::new(),
            theirs: Vec::new(),
            known: false,
        })
    })
}

impl Edit {
    pub fn new() -> Edit {
        let draft = RwSignal::new(Changeset::new(Value::Null));
        Edit {
            draft,
            base_rev: RwSignal::new(String::new()),
            texts: RwSignal::new(BTreeMap::new()),
            local: RwSignal::new(BTreeMap::new()),
            related: Memo::new(move |_| related_errors(draft)),
            plan: RwSignal::new(None),
            checking: RwSignal::new(false),
            check_error: RwSignal::new(None),
            conflict: RwSignal::new(None),
            conflict_revert: RwSignal::new(None),
            conflict_open: RwSignal::new(false),
            review: RwSignal::new(None),
            confirms: RwSignal::new(BTreeMap::new()),
            applying: RwSignal::new(false),
            outcome: RwSignal::new(None),
            reset_on_load: RwSignal::new(false),
            auto_apply: RwSignal::new(false),
            refused: RwSignal::new(std::collections::BTreeSet::new()),
            editing: RwSignal::new(BTreeMap::new()),
            after_apply: RwSignal::new(None),
            seq: StoredValue::new(0),
            carry: StoredValue::new(Vec::new()),
        }
    }

    pub fn dirty(&self) -> bool {
        self.draft.with(Changeset::dirty) || self.texts.with(|t| !t.is_empty())
    }

    pub fn ops(&self) -> Vec<Op> {
        self.texts.with(|t| self.draft.with(|d| ops_of(d, t)))
    }

    pub fn review_count(&self) -> usize {
        match self.review_plan() {
            Some(p) => crate::review::change_groups(&p).len(),
            None => self.pending(),
        }
    }

    fn open_edits(&self) -> Vec<(Path, Option<Value>)> {
        self.editing.with(|m| {
            m.iter()
                .map(|(p, snap)| (p.clone(), snap.clone()))
                .collect()
        })
    }

    fn saved_of(d: &Changeset, open: &[(Path, Option<Value>)]) -> Changeset {
        let mut c = d.clone();
        for (path, snap) in open {
            if path.is_empty() {
                let snap = snap.clone().unwrap_or(Value::Null);
                let mut keys: Vec<String> = Vec::new();
                for v in [&snap, d.draft()] {
                    if let Some(m) = v.as_object() {
                        keys.extend(
                            m.keys()
                                .filter(|k| !crate::route::SECTIONS.contains(&k.as_str()))
                                .cloned(),
                        );
                    }
                }
                keys.sort();
                keys.dedup();
                for k in keys {
                    restore(&mut c, std::slice::from_ref(&k), snap.get(&k));
                }
            } else {
                restore(&mut c, path, snap.as_ref());
            }
        }
        c
    }

    pub fn pending(&self) -> usize {
        let open = self.open_edits();
        let saved = self.draft.with(|d| {
            if open.is_empty() {
                d.changed_entities()
            } else {
                Self::saved_of(d, &open).changed_entities()
            }
        });
        saved + self.texts.with(BTreeMap::len)
    }

    pub fn saved_ops(&self) -> Vec<Op> {
        let open = self.open_edits();
        if open.is_empty() {
            return self.ops();
        }
        self.texts
            .with(|t| self.draft.with(|d| ops_of(&Self::saved_of(d, &open), t)))
    }

    fn unsaved_values(&self) -> Vec<(Path, Option<Value>)> {
        let open = self.open_edits();
        let mut out = Vec::new();
        self.draft.with_untracked(|d| {
            for (path, snap) in &open {
                if path.is_empty() {
                    if let Some(m) = d.draft().as_object() {
                        for (k, v) in m {
                            if crate::route::SECTIONS.contains(&k.as_str()) {
                                continue;
                            }
                            if snap.as_ref().and_then(|s| s.get(k)) != Some(v) {
                                out.push((vec![k.clone()], Some(v.clone())));
                            }
                        }
                    }
                    if let Some(m) = snap.as_ref().and_then(Value::as_object) {
                        for k in m.keys() {
                            if !crate::route::SECTIONS.contains(&k.as_str())
                                && d.draft().get(k).is_none()
                            {
                                out.push((vec![k.clone()], None));
                            }
                        }
                    }
                } else {
                    let now = burrow_admin_proto::value::get(d.draft(), path).cloned();
                    if now != *snap {
                        out.push((path.clone(), now));
                    }
                }
            }
        });
        out
    }

    pub fn value(&self, path: &[String]) -> Option<Value> {
        self.draft
            .with(|d| burrow_admin_proto::value::get(d.draft(), path).cloned())
    }

    pub fn peek(&self, path: &[String]) -> Option<Value> {
        self.draft
            .with_untracked(|d| burrow_admin_proto::value::get(d.draft(), path).cloned())
    }

    pub fn doc(&self) -> Value {
        self.draft.with(|d| d.draft().clone())
    }

    pub fn refuse(&self, path: &[String], refused: bool) {
        if self.refused.with_untracked(|r| r.contains(path)) != refused {
            self.refused.update(|r| {
                if refused {
                    r.insert(path.to_vec());
                } else {
                    r.remove(path);
                }
            });
        }
    }

    pub fn set(&self, path: &[String], v: Option<Value>) {
        if let [section, name, web, key] = path
            && section == ServiceEntry::SECTION
            && web == ServiceEntry::WEB.key()
        {
            let site = [section.clone(), name.clone(), web.clone()];
            let now = self.peek(&site);
            return self.set(
                &site,
                burrow_admin_logic::web::with_field(now.as_ref(), key, v),
            );
        }
        let refused = crate::grants::refuses(path, v.as_ref());
        self.refuse(path, refused);
        if refused {
            return;
        }
        self.draft.update(|d| d.set(path, v.unwrap_or(Value::Null)));
        self.changed();
    }

    pub fn remove_pruned(&self, path: &[String]) {
        self.draft.update(|d| d.remove_pruned(path));
        self.changed();
    }

    pub fn set_text(&self, path: &[String], text: Option<String>) {
        let refused = crate::grants::refuses_text(path, text.as_deref());
        self.refuse(path, refused);
        if refused {
            return;
        }
        self.texts.update(|t| match text {
            Some(x) => {
                t.insert(path.to_vec(), x);
            }
            None => {
                t.remove(path);
            }
        });
        self.changed();
    }

    pub fn add_secret(&self, path: &[String], handle: &str, fp: &str) {
        let mut secrets = burrow_admin_proto::value::one_or_many(self.value(path).as_ref());
        secrets.push(burrow_admin_logic::changeset::new_secret(handle, fp));
        let refused = crate::grants::refuses(path, Some(&Value::Array(secrets)));
        self.refuse(path, refused);
        if refused {
            return;
        }
        self.draft.update(|d| d.add_secret(path, handle, fp));
        self.changed();
    }

    pub fn set_password(&self, path: &[String], handle: &str) {
        self.draft.update(|d| d.set_password(path, handle));
        self.changed();
    }

    pub fn remove_secret(&self, path: &[String], fp: &str) {
        self.draft.update(|d| d.remove_secret(path, fp));
        self.changed();
    }

    pub fn rename(&self, path: &[String], to: &str) -> bool {
        let mut ok = false;
        self.draft.update(|d| ok = d.rename(path, to));
        if ok {
            self.editing.update(|m| {
                for (at, snap) in m.iter_mut() {
                    if let Some(s) = snap.as_mut() {
                        burrow_admin_logic::changeset::rename_under(s, at, path, to);
                    }
                }
            });
        }
        self.changed();
        ok
    }

    pub fn local_error(&self, path: &[String], err: Option<String>) {
        let same = self.local.with_untracked(|l| l.get(path) == err.as_ref());
        if !same {
            self.local.update(|l| match err {
                Some(e) => {
                    l.insert(path.to_vec(), e);
                }
                None => {
                    l.remove(path);
                }
            });
        }
    }

    pub fn discard(&self) {
        self.draft.update(Changeset::reset);
        self.texts.set(BTreeMap::new());
        self.local.set(BTreeMap::new());
        self.plan.set(None);
        self.end_conflict();
        self.check_error.set(None);
        self.confirms.set(BTreeMap::new());
        self.clear_refused();
        self.clear_edits();
        self.clear_changed();
    }
}

pub fn landing(
    plan: &Plan,
    renamed: impl Fn(&str, &str) -> Option<String>,
) -> Option<crate::route::Route> {
    use crate::route::Route;
    use burrow_admin_proto::plan::ChangeKind;
    let head = crate::review::change_groups(plan).into_iter().next()?.head;
    match head.path.as_slice() {
        [sec, name] if crate::route::SECTIONS.contains(&sec.as_str()) => Some(match head.kind {
            ChangeKind::Remove => Route::List(sec.clone()),
            ChangeKind::Rename => match renamed(sec, name) {
                Some(to) => Route::Detail(sec.clone(), to),
                None => Route::List(sec.clone()),
            },
            _ => Route::Detail(sec.clone(), name.clone()),
        }),
        _ => Some(Route::Settings),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::plan::{Impact, Level};
    use burrow_admin_proto::serde_json::json;

    fn p(s: &[&str]) -> Path {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn typed_confirmations_must_match_each_token() {
        let plan = Plan {
            impacts: vec![
                Impact {
                    level: Level::Danger,
                    code: burrow_admin_proto::codes::REMOVES_SERVICE.into(),
                    message: "x".into(),
                    confirm: Some("web".into()),
                },
                Impact {
                    level: Level::Warning,
                    code: "y".into(),
                    message: "y".into(),
                    confirm: None,
                },
            ],
            ..Plan::default()
        };
        let mut typed = BTreeMap::new();
        assert_eq!(missing_confirms(&plan, &typed), vec!["web".to_string()]);
        typed.insert("web".into(), "we".into());
        assert_eq!(missing_confirms(&plan, &typed).len(), 1);
        typed.insert("web".into(), " web ".into());
        assert!(missing_confirms(&plan, &typed).is_empty());
    }

    #[test]
    fn server_errors_become_outcomes() {
        let locked = ErrorBody {
            code: ErrorCode::LockedOut,
            message: "no".into(),
            body: json!({"reasons": [{"code": burrow_syntax::grant::SELF_DEMOTED, "message": "you would lose write access"}]}),
        };
        assert!(
            matches!(outcome_of(&locked), Outcome::Refused(r) if r[0].code == burrow_syntax::grant::SELF_DEMOTED)
        );
        let reload = ErrorBody {
            code: ErrorCode::ReloadFailed,
            message: "x".into(),
            body: json!({"restored": true, "reason": "port in use"}),
        };
        assert!(matches!(outcome_of(&reload), Outcome::ReloadFailed(r) if r.restored));
        let conflict = ErrorBody {
            code: ErrorCode::Conflict,
            message: "changed".into(),
            body: json!({"current": "b2s:1"}),
        };
        assert_eq!(
            conflict_of(&conflict).map(|c| c.current),
            Some("b2s:1".to_string())
        );
        assert!(conflict_of(&reload).is_none());
        assert!(matches!(
            outcome_of(&crate::api::unanswered()),
            Outcome::Unanswered
        ));
        let never_sent = ErrorBody {
            code: ErrorCode::Unknown,
            message: "not connected to the server".into(),
            body: Value::Null,
        };
        assert!(matches!(outcome_of(&never_sent), Outcome::Failed { .. }));
        assert!(matches!(
            revert_outcome(&crate::api::unanswered()),
            Outcome::Failed { message, .. } if message.contains("did not finish") && message.contains("nothing was applied")
        ));
        assert!(matches!(revert_outcome(&reload), Outcome::ReloadFailed(_)));
    }

    #[test]
    fn a_discard_or_a_fresh_base_clears_the_refused_marks() {
        let e = Edit::new();
        e.refused.set([p(&["visitors", "lap", "admin"])].into());
        e.discard();
        assert!(e.refused.get_untracked().is_empty());
        e.refused.set([p(&["visitors", "lap", "admin"])].into());
        e.sync_base(&json!({"visitors": {}}), "b2s:1");
        assert!(e.refused.get_untracked().is_empty());
    }

    #[test]
    fn the_review_count_leaves_out_edits_not_yet_added_to_the_changes() {
        let e = Edit::new();
        e.sync_base(
            &json!({"pool": {"min_idle": 1}, "visitors": {"lap": {"max_connections": 5}, "bob": {}}}),
            "b2s:1",
        );
        let lap = p(&["visitors", "lap"]);
        let bob = p(&["visitors", "bob"]);
        e.start_edit(&lap);
        e.draft
            .update(|d| d.set(&p(&["visitors", "lap", "max_connections"]), json!(9)));
        assert_eq!(e.pending(), 0, "an open, unsaved edit is not a change yet");
        e.stop_edit(&lap);
        assert_eq!(e.pending(), 1);
        e.start_edit(&lap);
        e.draft
            .update(|d| d.set(&p(&["visitors", "lap", "max_connections"]), json!(11)));
        assert_eq!(e.pending(), 1, "what was added before still counts");
        e.start_edit(&bob);
        e.draft
            .update(|d| d.set(&p(&["visitors", "bob", "max_connections"]), json!(3)));
        assert_eq!(e.pending(), 1);
        e.start_edit(&[]);
        e.draft
            .update(|d| d.set(&p(&["pool", "min_idle"]), json!(2)));
        assert_eq!(e.pending(), 1, "open settings edits do not count either");
        let saved: Vec<String> = e
            .saved_ops()
            .iter()
            .filter_map(|o| o.path().map(|p| p.join(".")))
            .collect();
        assert_eq!(saved, ["visitors.lap.max_connections"]);
        e.carry.set_value(e.unsaved_values());
        e.reset_on_load.set(true);
        e.sync_base(
            &json!({"pool": {"min_idle": 1}, "visitors": {"lap": {"max_connections": 9}, "bob": {}}}),
            "b2s:2",
        );
        assert!(
            e.unsaved(&lap) && e.unsaved(&bob),
            "open edits survive the apply"
        );
        assert_eq!(
            e.value(&p(&["visitors", "lap", "max_connections"])),
            Some(json!(11))
        );
        assert_eq!(e.value(&p(&["pool", "min_idle"])), Some(json!(2)));
        assert_eq!(e.pending(), 0);
    }

    #[test]
    fn edit_state_is_kept_per_entity_until_saved_discarded_or_reset() {
        let e = Edit::new();
        e.sync_base(
            &json!({"visitors": {"lap": {"max_connections": 5}, "bob": {}}}),
            "b2s:1",
        );
        let lap = p(&["visitors", "lap"]);
        let bob = p(&["visitors", "bob"]);
        e.start_edit(&lap);
        e.start_edit(&bob);
        assert!(e.is_editing(&lap) && e.is_editing(&bob));
        e.draft
            .update(|d| d.set(&p(&["visitors", "lap", "max_connections"]), json!(9)));
        assert!(e.unsaved(&lap));
        assert!(!e.unsaved(&bob));
        e.start_edit(&lap);
        e.draft
            .update(|d| d.set(&lap, json!({"max_connections": 5})));
        assert!(!e.unsaved(&lap), "a second start keeps the first snapshot");
        e.stop_edit(&bob);
        assert!(!e.is_editing(&bob));
        e.discard();
        assert!(
            !e.is_editing(&lap),
            "a discard of the draft ends every edit"
        );
    }

    #[test]
    fn a_reload_of_the_same_or_a_newer_base_keeps_an_open_edit() {
        let e = Edit::new();
        let lap = p(&["visitors", "lap"]);
        e.sync_base(
            &json!({"visitors": {"lap": {"max_connections": 5}}}),
            "b2s:1",
        );
        e.start_edit(&lap);
        e.local.update(|l| {
            l.insert(
                p(&["visitors", "lap", "max_connections"]),
                "not a number".into(),
            );
        });
        e.sync_base(
            &json!({"visitors": {"lap": {"max_connections": 5}}}),
            "b2s:1",
        );
        assert!(
            e.is_editing(&lap),
            "the 5 s reload of the same rev keeps the edit"
        );
        assert_eq!(e.local.get_untracked().len(), 1, "and the typed problem");
        e.sync_base(
            &json!({"visitors": {"lap": {"max_connections": 7}}}),
            "b2s:2",
        );
        assert!(
            e.is_editing(&lap),
            "a change elsewhere keeps an edit with nothing typed"
        );
        assert!(!e.unsaved(&lap), "its snapshot follows the new base");
        e.reset_on_load.set(true);
        e.sync_base(
            &json!({"visitors": {"lap": {"max_connections": 7}}}),
            "b2s:3",
        );
        assert!(!e.is_editing(&lap), "an apply ends the edits");
    }

    #[test]
    fn after_apply_the_page_shows_what_changed() {
        use crate::route::Route;
        use burrow_admin_proto::plan::{Change, ChangeKind};
        let plan = |kind, path: &[&str]| Plan {
            changes: vec![Change {
                kind,
                path: p(path),
                summary: String::new(),
                part_of: None,
            }],
            ..Plan::default()
        };
        let none = |_: &str, _: &str| None;
        assert_eq!(
            landing(&plan(ChangeKind::Add, &["visitors", "phone"]), none),
            Some(Route::Detail("visitors".into(), "phone".into()))
        );
        assert_eq!(
            landing(&plan(ChangeKind::Remove, &["visitors", "phone"]), none),
            Some(Route::List("visitors".into()))
        );
        assert_eq!(
            landing(
                &plan(ChangeKind::Rename, &["clients", "home"]),
                |_, _| Some("house".into())
            ),
            Some(Route::Detail("clients".into(), "house".into()))
        );
        assert_eq!(
            landing(&plan(ChangeKind::Set, &["pool"]), none),
            Some(Route::Settings)
        );
        assert_eq!(landing(&Plan::default(), none), None);
    }

    #[test]
    fn hand_written_text_goes_after_the_draft_ops() {
        let mut d = Changeset::new(json!({"a": 1}));
        d.set(&p(&["a"]), json!(2));
        let mut t = BTreeMap::new();
        t.insert(p(&["geo"]), "x: 1".to_string());
        let ops = ops_of(&d, &t);
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[1], Op::SetText { path, .. } if path == &p(&["geo"])));
    }
}
