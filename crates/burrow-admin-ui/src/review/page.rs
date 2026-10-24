use burrow_admin_proto::value::get;
use burrow_ui_kit::{
    Button, Callout, Dialog, EmptyState, Icon, Skeleton, Tone, button_class, tag_class,
};
use leptos::prelude::*;

use crate::edit::Outcome;
use crate::route::{self, Link, Route};
use crate::screens::{DetailPane, ListPane, Row, master_detail};
use crate::store::{Store, short_rev, store};

use super::detail::{detail, outcome_view};
use super::{change_groups, group_line, kind_look};

#[component]
pub fn ReviewPage() -> impl IntoView {
    let s = store();
    let e = s.edit;
    let reverting = e
        .review
        .with_untracked(|r| r.as_ref().is_some_and(|r| r.revert_of.is_some()));
    if !reverting && untrack(|| e.dirty()) {
        e.open_review();
    }
    Effect::new(move |prev: Option<Option<String>>| {
        let rev = e.outcome.with(|o| match o {
            Some(Outcome::Applied(a)) => Some(a.rev.clone()),
            _ => None,
        });
        if let Some(r) = &rev
            && prev.is_some()
            && prev.as_ref() != Some(&rev)
        {
            s.toast(Tone::Success, format!("Applied rev {}", short_rev(r)), None);
            let shown = e.outcome.with_untracked(|o| match o {
                Some(Outcome::Applied(a)) => a
                    .secrets
                    .first()
                    .and_then(|x| Some((x.path.first()?.clone(), x.path.get(1)?.clone()))),
                _ => None,
            });
            match shown {
                Some((sec, name)) => route::go(Route::Detail(sec, name)),
                None => route::go(
                    e.after_apply
                        .get_untracked()
                        .unwrap_or_else(|| s.prev.get_untracked()),
                ),
            }
        }
        rev
    });
    on_cleanup(move || {
        let _ = e.auto_apply.try_set(false);
        if e.review
            .try_with_untracked(|r| r.as_ref().is_some_and(|r| r.revert_of.is_some()))
            .unwrap_or(false)
        {
            let _ = e.review.try_set(None);
        }
    });
    let plan = Memo::new(move |_| e.review_plan());
    Effect::new(move |_| {
        if !e.auto_apply.get() {
            return;
        }
        if let Some(p) = plan.get() {
            e.auto_apply.set(false);
            if p.can_apply() && p.confirms().is_empty() {
                e.apply(s);
            }
        }
    });
    let revert = Memo::new(move |_| {
        e.review
            .with(|r| r.as_ref().is_some_and(|r| r.revert_of.is_some()))
    });
    let open = Memo::new(move |_| e.review.with(Option::is_some));
    let rows = Signal::derive(move || {
        let Some(p) = plan.get() else {
            return Vec::new();
        };
        let draft_has =
            |path: &[String]| e.draft.with_untracked(|d| get(d.draft(), path).is_some());
        let out: Vec<Row> = change_groups(&p)
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let (icon, tone) = kind_look(g.head.kind);
                let entity = g.head.path.len() == 2
                    && crate::route::SECTIONS.contains(&g.head.path[0].as_str());
                let to = if entity && !revert.get_untracked() && draft_has(&g.head.path) {
                    Route::Detail(g.head.path[0].clone(), g.head.path[1].clone())
                } else {
                    Route::Review
                };
                Row {
                    key: format!("{i}"),
                    to,
                    icon,
                    dot: Some(tone),
                    title: g.head.summary.clone(),
                    line2: group_line(g),
                    group: "Changes".into(),
                    ..Row::default()
                }
            })
            .collect();
        out
    });
    let count = Signal::stored(String::new());
    let list = view! {
        <ListPane
            title="Review"
            count=count
            search_label="Search changes"
            filters=Vec::new()
            groups=vec!["Changes"]
            no_counts=true
            empty=(String::new(), String::new())
            rows=rows
            selected=Signal::stored(None::<String>)
            testid="review-list"
        />
    }
    .into_any();
    let body = move || {
        if !open.get() {
            return view! {
                {outcome_view()}
                <EmptyState icon="check" title="Nothing to review" message="Your changes show up here before they are applied." testid="review-empty">
                    <Link to=Route::Overview class=button_class("secondary", false)>"Go to the overview"</Link>
                </EmptyState>
            }
            .into_any();
        }
        if !revert.get() && !e.dirty() {
            return view! {
                {outcome_view()}
                <EmptyState icon="check" title="Nothing to apply" message="Your draft has no changes left." testid="review-empty">
                    <Link to=s.prev.get_untracked() class=button_class("secondary", false)>"Back"</Link>
                </EmptyState>
            }
            .into_any();
        }
        if !revert.get()
            && let Some((base, now)) = stale(s)
        {
            return view! {
                <Callout tone=Tone::Warning title=format!("Based on rev {}; the file is now rev {}", short_rev(&base), short_rev(&now)) testid="review-stale">
                    <p class="mb-2.5">"The config changed on the server after you started. Your draft is kept; check it on the new rev to see what it applies."</p>
                    <Button label=format!("Review my draft on rev {}", short_rev(&now)) variant="primary" icon="refresh" small=true on_click=move |_| recheck(s)/>
                </Callout>
            }
            .into_any();
        }
        match plan.get() {
            None => view! {
                <div data-testid="review-loading">
                    {outcome_view()}
                    <p class="mb-2 text-text-muted">"Checking your change on the server…"</p>
                    <Skeleton lines=4/>
                </div>
            }
            .into_any(),
            Some(p) => view! { {outcome_view()} {detail(p, revert.get())} }.into_any(),
        }
    };
    let detail = view! { <DetailPane>{body}</DetailPane> }.into_any();
    master_detail(list, detail)
}

pub fn stale(s: Store) -> Option<(String, String)> {
    let e = s.edit;
    if !e.dirty()
        || e.review
            .with(|r| r.as_ref().is_some_and(|r| r.revert_of.is_some()))
    {
        return None;
    }
    let base = e.base_rev.get();
    let now = s.state.with(|st| st.as_ref().map(|st| st.rev.clone()))?;
    (!base.is_empty() && base != now).then_some((base, now))
}

pub fn recheck(s: Store) {
    let e = s.edit;
    e.keep_open_edits();
    let revert = e.conflict_revert.get_untracked();
    e.conflict_revert.set(None);
    e.end_conflict();
    crate::api::request(
        burrow_admin_proto::msg::req::STATE,
        burrow_admin_proto::serde_json::json!({}),
        move |r| {
            if let Ok(Ok(state)) =
                r.map(burrow_admin_proto::serde_json::from_value::<burrow_admin_proto::msg::State>)
            {
                s.state.set(Some(std::sync::Arc::new(state)));
                match revert.clone() {
                    Some(to) => s.edit.open_revert(s, to),
                    None => {
                        s.edit.rebase(s);
                        s.edit.open_review();
                        route::go_in(s, Route::Review);
                    }
                }
            }
        },
    );
}

#[component]
pub fn ConflictDialog() -> impl IntoView {
    let s = store();
    let e = s.edit;
    let open = e.conflict_open;
    let current = move || {
        e.conflict.with(|c| {
            c.as_ref()
                .map(|c| short_rev(&c.current))
                .unwrap_or_default()
        })
    };
    let by = move || {
        s.changed.with(|c| match c {
            Some(c) if !c.by.is_empty() && c.by != "file" => c.by.clone(),
            _ => "hand edit".to_string(),
        })
    };
    let base = move || short_rev(&e.base_rev.get());
    let theirs_paths = move || {
        e.conflict.with(|c| {
            c.as_ref()
                .map(|c| c.theirs.iter().map(|x| x.path.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    };
    let theirs = move || {
        let tag = by();
        e.conflict.with(|c| {
            c.as_ref().map(|c| {
                c.theirs
                    .iter()
                    .map(|ch| {
                        view! {
                            <li class="flex items-center gap-2.5 border-b border-rowline py-2.5 last:border-b-0">
                                <span class=tag_class(Tone::Warning)>{tag.clone()}</span>
                                <span>{ch.summary.clone()}</span>
                            </li>
                        }
                    })
                    .collect_view()
            })
        })
    };
    let mine = move || {
        let theirs = theirs_paths();
        e.plan.with(|p| {
            p.as_ref()
                .map(|p| {
                    p.changes
                        .iter()
                        .map(|ch| {
                            let overlap = theirs.iter().any(|t| t.starts_with(&ch.path) || ch.path.starts_with(t));
                            view! {
                                <li class="flex items-center gap-2.5 border-b border-rowline py-2.5 last:border-b-0">
                                    <span class="text-accent-ink"><Icon name="edit" small=true/></span>
                                    <span>{ch.summary.clone()}</span>
                                    {overlap.then(|| view! { <span class=format!("ml-auto {}", tag_class(Tone::Warning))><Icon name="alert" small=true/>"also changed on the server"</span> })}
                                </li>
                            }
                        })
                        .collect_view()
                })
        })
    };
    let n_mine = move || e.review_count();
    let sub = Signal::derive(move || {
        format!(
            "Rev {} was written by a {} while you were editing rev {}. Nothing was applied. Your draft is kept.",
            current(),
            if by() == "hand edit" {
                "hand edit".to_string()
            } else {
                format!("visitor {}", by())
            },
            base()
        )
    });
    let label = "mb-1 text-[11px] font-bold tracking-[0.07em] text-text-faint uppercase";
    view! {
        <Dialog open=open title="The config changed on the server" sub=sub icon="refresh" tone=Tone::Warning wide=true testid="conflict">
            {move || e.conflict.with(|c| c.as_ref().is_some_and(|c| !c.theirs.is_empty())).then(|| view! {
                <h3 class=label>{move || format!("Changed on the server · rev {}", current())}</h3>
                <ul class="mb-4 text-sm" data-testid="conflict-theirs">{theirs}</ul>
            })}
            <h3 class=label>{move || format!("Your draft · {} change{}", n_mine(), if n_mine() == 1 { "" } else { "s" })}</h3>
            <ul class="mb-5 text-sm" data-testid="conflict-mine">{mine}</ul>
            <div class="flex flex-wrap items-center gap-2">
                <Button label="Discard my draft" variant="ghost-danger" testid="conflict-discard" on_click=move |_| {
                    e.conflict_revert.set(None);
                    e.discard();
                    crate::api::load_state(s);
                }/>
                <span class="flex-1"></span>
                <Button label=Signal::derive(move || format!("View rev {}", current())) variant="secondary" testid="conflict-view" on_click=move |_| {
                    let rev = e.conflict.with_untracked(|c| c.as_ref().map(|c| c.current.clone()));
                    e.conflict_revert.set(None);
                    e.end_conflict();
                    route::go(Route::Activity(rev));
                }/>
                <Button label=Signal::derive(move || format!("Review my draft on rev {}", current())) variant="primary" icon="refresh" testid="conflict-recheck" on_click=move |_| recheck(s)/>
            </div>
        </Dialog>
    }
}
