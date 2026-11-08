use std::collections::BTreeMap;
use std::sync::Arc;

use burrow_admin_proto::msg::{self, ErrorBody, ErrorCode};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::{Applied, Plan};
use burrow_admin_proto::serde_json::{self, Value};
use leptos::prelude::*;

use crate::api;
use crate::store::Store;

use super::{
    Edit, Outcome, Review, Source, VALIDATE_AFTER, conflict_of, landing, outcome_of, revert_outcome,
};

fn text_changed(store: Store) {
    store.text.fetch();
    api::load_state(store);
    let (route, query) = crate::route::file_tab();
    crate::route::go_query(route, query);
}

fn protocol_problem(e: serde_json::Error) -> ErrorBody {
    ErrorBody {
        code: ErrorCode::Unknown,
        message: crate::store::store().protocol_problem("plan", e),
        body: Value::Null,
    }
}

impl Edit {
    pub(super) fn changed(&self) {
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
        self.check_error.set(None);
        let body = api::body(&msg::Validate {
            base: self.base_rev.get_untracked(),
            ops: self.ops(),
        });
        api::request_within(msg::req::VALIDATE, body, api::CHECK_WITHIN, move |r| {
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
                        me.check_error.set(Some(protocol_problem(e)));
                    }
                },
                Err(e) => match conflict_of(&e) {
                    Some(c) => {
                        me.conflict_revert.set(None);
                        me.raise(c, false);
                    }
                    None => me.check_error.set(Some(e)),
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
        let ops = untrack(|| self.saved_ops());
        let own = ops != untrack(|| self.ops());
        let base = self.base_rev.get_untracked();
        self.confirms.set(BTreeMap::new());
        self.outcome.set(None);
        self.review.set(Some(Review {
            title: "Review changes".into(),
            ops: ops.clone(),
            base: base.clone(),
            plan: if own { None } else { self.plan.get_untracked() },
            source: Source::Draft,
            own,
        }));
        if !own {
            if self.plan.with_untracked(Option::is_none) {
                self.revalidate_now();
            }
            return;
        }
        let me = *self;
        self.check_error.set(None);
        api::request_within(
            msg::req::VALIDATE,
            api::body(&msg::Validate { base, ops }),
            api::CHECK_WITHIN,
            move |r| match r
                .and_then(|v| serde_json::from_value::<Plan>(v).map_err(protocol_problem))
            {
                Ok(p) => {
                    me.review.update(|rv| {
                        if let Some(rv) = rv.as_mut().filter(|rv| rv.own) {
                            rv.plan = Some(Arc::new(p));
                        }
                    });
                }
                Err(e) => {
                    if let Some(c) = conflict_of(&e) {
                        me.conflict_revert.set(None);
                        me.raise(c, false);
                    } else {
                        me.check_error.set(Some(e));
                    }
                }
            },
        );
    }

    fn open_own(&self, review: Review) {
        self.confirms.set(BTreeMap::new());
        self.outcome.set(None);
        self.review.set(Some(review));
    }

    fn check_own(
        &self,
        req: &'static str,
        body: Value,
        source: Source,
        failed: impl FnOnce(ErrorBody) + 'static,
    ) {
        let me = *self;
        api::request_within(req, body, api::CHECK_WITHIN, move |r| {
            let still = me
                .review
                .with_untracked(|rv| rv.as_ref().is_some_and(|rv| rv.source == source));
            if !still {
                return;
            }
            match r.and_then(|v| serde_json::from_value::<Plan>(v).map_err(protocol_problem)) {
                Ok(p) => me.review.update(|rv| {
                    if let Some(rv) = rv {
                        rv.plan = Some(Arc::new(p));
                    }
                }),
                Err(e) => failed(e),
            }
        });
    }

    pub fn open_revert(&self, store: Store, to: String) {
        let base = store
            .state
            .with_untracked(|s| s.as_ref().map(|s| s.rev.clone()))
            .unwrap_or_default();
        self.open_own(Review {
            title: format!("Revert to version {to}"),
            ops: vec![Op::ReplaceDocument {
                from_history: to.clone(),
            }],
            base: base.clone(),
            plan: None,
            source: Source::Revert(to.clone()),
            own: true,
        });
        let me = *self;
        let body = api::body(&msg::Revert {
            to: to.clone(),
            base,
        });
        self.check_own(
            msg::req::REVERT,
            body,
            Source::Revert(to.clone()),
            move |e| {
                me.review.set(None);
                match conflict_of(&e) {
                    Some(c) => {
                        me.conflict_revert.set(Some(to));
                        me.raise(c, true);
                    }
                    None => me.outcome.set(Some(revert_outcome(&e))),
                }
            },
        );
    }

    pub fn open_text(&self, store: Store) {
        let ops = store.text.ops();
        if ops.is_empty() {
            return;
        }
        let base = store.text.base();
        self.open_own(Review {
            title: "Review changes".into(),
            ops: ops.clone(),
            base: base.clone(),
            plan: None,
            source: Source::Text,
            own: true,
        });
        crate::route::go_in(store, crate::route::Route::Review);
        let me = *self;
        let body = api::body(&msg::Validate { base, ops });
        self.check_own(msg::req::VALIDATE, body, Source::Text, move |e| {
            me.review.set(None);
            match outcome_of(&e) {
                _ if conflict_of(&e).is_some() => text_changed(store),
                Outcome::Unanswered => me.outcome.set(Some(Outcome::Unanswered)),
                _ => {
                    store.text.problems.set(super::text::refusal(&e));
                    let (route, query) = crate::route::file_tab();
                    crate::route::go_query(route, query);
                }
            }
        });
    }

    pub fn review_plan(&self) -> Option<Arc<Plan>> {
        self.review.with(|r| {
            r.as_ref().and_then(|r| {
                if r.own {
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
        let ops = if review.own {
            review.ops.clone()
        } else {
            self.ops()
        };
        let base = if review.own {
            review.base.clone()
        } else {
            self.base_rev.get_untracked()
        };
        let carry = if review.standalone() {
            Vec::new()
        } else {
            self.unsaved_values()
        };
        let confirm: Vec<String> = plan.confirms().into_iter().map(str::to_string).collect();
        let target = match &review.source {
            Source::Revert(_) => Some((crate::route::Route::Activity(None), None)),
            Source::Text => Some(crate::route::file_tab()),
            Source::Draft => self
                .draft
                .with_untracked(|d| landing(&plan, |sec, name| d.renamed_to(sec, name)))
                .map(|r| (r, None)),
        };
        self.after_apply.set(target);
        self.applying.set(true);
        self.outcome.set(None);
        let me = *self;
        let address = store.server_addr.get_untracked();
        let body = api::body(&msg::Apply {
            base,
            ops,
            plan: plan.plan.clone(),
            confirm,
            address: (!address.is_empty()).then_some(address),
        });
        api::request(msg::req::APPLY, body, move |r| {
            me.applying.set(false);
            match r {
                Ok(v) => {
                    let applied = serde_json::from_value::<Applied>(v).unwrap_or_default();
                    me.outcome.set(Some(Outcome::Applied(applied)));
                    me.review.set(None);
                    if review.source == Source::Text {
                        store.text.close();
                    }
                    store.changed.set(None);
                    me.carry.set_value(carry.clone());
                    me.reset_on_load.set(true);
                    api::load_state(store);
                    api::load_history(store);
                }
                Err(e) => match conflict_of(&e) {
                    Some(_) if review.source == Source::Text => {
                        me.review.set(None);
                        text_changed(store);
                    }
                    Some(c) => {
                        me.review.set(None);
                        me.conflict_revert.set(review.source.revert_of());
                        me.raise(c, true);
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
