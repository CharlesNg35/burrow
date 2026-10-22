use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use burrow_admin_logic::changeset::Changeset;
use burrow_admin_proto::Path;
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_admin_proto::msg::{self, ErrorBody, ErrorCode};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::{Applied, Conflict, Plan, Refusal, ReloadFailed};
use burrow_admin_proto::schema::SemType;
use burrow_admin_proto::serde_json::{self, Value, json};
use leptos::prelude::*;

use crate::api;
use crate::store::Store;

pub const VALIDATE_AFTER: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub title: String,
    pub ops: Vec<Op>,
    pub base: String,
    pub plan: Option<Arc<Plan>>,
    pub revert_of: Option<String>,
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
    pub check_error: RwSignal<Option<String>>,
    pub conflict: RwSignal<Option<Conflict>>,
    pub conflict_revert: RwSignal<Option<String>>,
    pub review: RwSignal<Option<Review>>,
    pub confirms: RwSignal<BTreeMap<String, String>>,
    pub applying: RwSignal<bool>,
    pub outcome: RwSignal<Option<Outcome>>,
    pub reset_on_load: RwSignal<bool>,
    pub auto_apply: RwSignal<bool>,
    pub refused: RwSignal<std::collections::BTreeSet<Path>>,
    seq: StoredValue<u64>,
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

pub fn diags_at<'a>(plan: Option<&'a Plan>, path: &[String]) -> Vec<&'a Diag> {
    plan.map(|p| p.diags.iter().filter(|d| d.concerns(path)).collect())
        .unwrap_or_default()
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
            serde_json::from_value(err.body.get("reasons").cloned().unwrap_or(Value::Null))
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
            current: err
                .body
                .get("current")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
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
            review: RwSignal::new(None),
            confirms: RwSignal::new(BTreeMap::new()),
            applying: RwSignal::new(false),
            outcome: RwSignal::new(None),
            reset_on_load: RwSignal::new(false),
            auto_apply: RwSignal::new(false),
            refused: RwSignal::new(std::collections::BTreeSet::new()),
            seq: StoredValue::new(0),
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
            Some(p) => p.changes.len().max(1),
            None => self.pending(),
        }
    }

    pub fn pending(&self) -> usize {
        self.draft.with(Changeset::changed_entities) + self.texts.with(BTreeMap::len)
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
        let refused = crate::grants::refuses(path, v.as_ref());
        self.refuse(path, refused);
        if refused {
            return;
        }
        self.draft.update(|d| d.set(path, v.unwrap_or(Value::Null)));
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

    pub fn remove_secret(&self, path: &[String], fp: &str) {
        self.draft.update(|d| d.remove_secret(path, fp));
        self.changed();
    }

    pub fn rename(&self, path: &[String], to: &str) -> bool {
        let mut ok = false;
        self.draft.update(|d| ok = d.rename(path, to));
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
        self.conflict.set(None);
        self.check_error.set(None);
        self.confirms.set(BTreeMap::new());
        self.clear_refused();
    }

    fn clear_refused(&self) {
        if self.refused.with_untracked(|r| !r.is_empty()) {
            self.refused.set(std::collections::BTreeSet::new());
        }
    }

    pub fn sync_base(&self, values: &Value, rev: &str) {
        let reset = self.reset_on_load.get_untracked();
        if reset || !self.dirty() || self.base_rev.get_untracked().is_empty() {
            self.draft.set(Changeset::new(values.clone()));
            self.texts.set(BTreeMap::new());
            self.local.set(BTreeMap::new());
            self.clear_refused();
            self.plan.set(None);
            if self.conflict_revert.with_untracked(Option::is_none) {
                self.conflict.set(None);
            }
            self.base_rev.set(rev.to_string());
            self.reset_on_load.set(false);
        } else if self.base_rev.get_untracked() != rev && self.conflict.get_untracked().is_none() {
            self.conflict_revert.set(None);
            self.conflict.set(Some(Conflict {
                current: rev.to_string(),
                theirs: Vec::new(),
                known: false,
            }));
        }
    }

    pub fn rebase(&self, store: Store) {
        let Some(state) = store.state.get_untracked() else {
            return;
        };
        self.draft.update(|d| d.rebase(state.values.clone()));
        self.base_rev.set(state.rev.clone());
        self.conflict.set(None);
        self.clear_refused();
        self.changed();
    }

    fn changed(&self) {
        let n = self.seq.get_value() + 1;
        self.seq.set_value(n);
        self.plan.set(None);
        self.auto_apply.set(false);
        let me = *self;
        set_timeout(move || me.validate(n), VALIDATE_AFTER);
    }

    fn validate(&self, n: u64) {
        if self.seq.get_value() != n {
            return;
        }
        if !self.dirty() {
            self.checking.set(false);
            return;
        }
        let me = *self;
        self.checking.set(true);
        let body = json!({ "base": self.base_rev.get_untracked(), "ops": self.ops() });
        api::request(msg::req::VALIDATE, body, move |r| {
            if me.seq.get_value() != n {
                return;
            }
            me.checking.set(false);
            if r.is_err() {
                me.auto_apply.set(false);
            }
            match r {
                Ok(v) => match serde_json::from_value::<Plan>(v) {
                    Ok(p) => {
                        me.check_error.set(None);
                        me.plan.set(Some(Arc::new(p)));
                    }
                    Err(e) => {
                        me.auto_apply.set(false);
                        me.check_error.set(Some(format!("unreadable plan: {e}")));
                    }
                },
                Err(e) => match conflict_of(&e) {
                    Some(c) => {
                        me.conflict_revert.set(None);
                        me.conflict.set(Some(c));
                    }
                    None => me.check_error.set(Some(e.message)),
                },
            }
        });
    }

    pub fn revalidate_now(&self) {
        let n = self.seq.get_value() + 1;
        self.seq.set_value(n);
        self.validate(n);
    }

    pub fn open_review(&self) {
        let ops = self.ops();
        self.confirms.set(BTreeMap::new());
        self.outcome.set(None);
        self.review.set(Some(Review {
            title: "Review changes".into(),
            ops,
            base: self.base_rev.get_untracked(),
            plan: self.plan.get_untracked(),
            revert_of: None,
        }));
        if self.plan.with_untracked(Option::is_none) {
            self.revalidate_now();
        }
    }

    pub fn open_revert(&self, store: Store, to: String) {
        let base = store
            .state
            .with_untracked(|s| s.as_ref().map(|s| s.rev.clone()))
            .unwrap_or_default();
        self.confirms.set(BTreeMap::new());
        self.outcome.set(None);
        self.review.set(Some(Review {
            title: format!("Revert to version {to}"),
            ops: vec![Op::ReplaceDocument {
                from_history: to.clone(),
            }],
            base: base.clone(),
            plan: None,
            revert_of: Some(to.clone()),
        }));
        let me = *self;
        api::request(
            msg::req::REVERT,
            json!({ "to": to, "base": base }),
            move |r| match r.and_then(|v| {
                serde_json::from_value::<Plan>(v).map_err(|e| ErrorBody {
                    code: ErrorCode::Unknown,
                    message: format!("unreadable plan: {e}"),
                    body: Value::Null,
                })
            }) {
                Ok(p) => {
                    me.review.update(|rv| {
                        if let Some(rv) = rv {
                            rv.plan = Some(Arc::new(p));
                        }
                    });
                }
                Err(e) => match conflict_of(&e) {
                    Some(c) => {
                        me.review.set(None);
                        me.conflict_revert.set(Some(to.clone()));
                        me.conflict.set(Some(c));
                    }
                    None => {
                        me.review.set(None);
                        me.outcome.set(Some(revert_outcome(&e)));
                    }
                },
            },
        );
    }

    pub fn review_plan(&self) -> Option<Arc<Plan>> {
        self.review.with(|r| {
            r.as_ref().and_then(|r| {
                if r.revert_of.is_some() {
                    r.plan.clone()
                } else {
                    self.plan.get()
                }
            })
        })
    }

    pub fn apply(&self, store: Store) {
        let Some(review) = self.review.get_untracked() else {
            return;
        };
        let Some(plan) = self.review_plan() else {
            return;
        };
        let ops = if review.revert_of.is_some() {
            review.ops.clone()
        } else {
            self.ops()
        };
        let base = if review.revert_of.is_some() {
            review.base.clone()
        } else {
            self.base_rev.get_untracked()
        };
        let confirm: Vec<String> = plan.confirms().into_iter().map(str::to_string).collect();
        self.applying.set(true);
        self.outcome.set(None);
        let me = *self;
        let body = json!({ "base": base, "ops": ops, "plan": plan.plan, "confirm": confirm });
        api::request(msg::req::APPLY, body, move |r| {
            me.applying.set(false);
            match r {
                Ok(v) => {
                    let applied = serde_json::from_value::<Applied>(v).unwrap_or_default();
                    me.outcome.set(Some(Outcome::Applied(applied)));
                    me.review.set(None);
                    me.reset_on_load.set(true);
                    api::load_state(store);
                    api::load_history(store);
                }
                Err(e) => match conflict_of(&e) {
                    Some(c) => {
                        me.review.set(None);
                        me.conflict_revert.set(review.revert_of.clone());
                        me.conflict.set(Some(c));
                        api::load_state(store);
                    }
                    None => {
                        if e.code == ErrorCode::ReloadFailed {
                            me.review.set(None);
                            api::load_state(store);
                        }
                        me.outcome.set(Some(outcome_of(&e)));
                    }
                },
            }
        });
    }
}

fn related_errors(draft: RwSignal<Changeset>) -> BTreeMap<Path, String> {
    let Some(schema) = crate::store::try_store().and_then(|s| s.schema.get()) else {
        return BTreeMap::new();
    };
    draft.with(|d| entity_errors(&schema, d.draft()))
}

fn entity_errors(
    schema: &burrow_admin_proto::schema::Schema,
    doc: &Value,
) -> BTreeMap<Path, String> {
    let mut out = BTreeMap::new();
    for info in &schema.entities {
        let Some(node) = schema.root.at(&info.path) else {
            continue;
        };
        let SemType::Map(m) = &burrow_admin_logic::tree::inner(node).sem else {
            continue;
        };
        let Some(items) =
            burrow_admin_proto::value::get(doc, &info.path).and_then(Value::as_object)
        else {
            continue;
        };
        for (name, value) in items {
            for (key, msg) in burrow_admin_logic::relations::entity_problems(&m.value, value) {
                let mut p = info.path.clone();
                p.extend([name.clone(), key]);
                out.entry(p).or_insert(msg);
            }
        }
    }
    out
}

pub fn severity_tone(d: &Diag) -> burrow_ui_kit::Tone {
    match d.severity {
        Severity::Error => burrow_ui_kit::Tone::Danger,
        Severity::Warning => burrow_ui_kit::Tone::Warning,
        Severity::Info => burrow_ui_kit::Tone::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::plan::{Impact, Level};

    fn p(s: &[&str]) -> Path {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn typed_confirmations_must_match_each_token() {
        let plan = Plan {
            impacts: vec![
                Impact {
                    level: Level::Danger,
                    code: "removes-service".into(),
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
            body: json!({"reasons": [{"code": "self-demoted", "message": "you would lose write access"}]}),
        };
        assert!(matches!(outcome_of(&locked), Outcome::Refused(r) if r[0].code == "self-demoted"));
        let reload = ErrorBody {
            code: ErrorCode::ReloadFailed,
            message: "x".into(),
            body: json!({"restored": true, "reason": "port in use"}),
        };
        assert!(matches!(outcome_of(&reload), Outcome::ReloadFailed(r) if r.restored));
        let conflict = ErrorBody {
            code: ErrorCode::Conflict,
            message: "changed".into(),
            body: json!({"current": "b2s:1", "plan_changed": true}),
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
