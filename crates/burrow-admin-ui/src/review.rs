use burrow_admin_proto::plan::{ChangeKind, Level};
use burrow_ui_kit::{
    Banner, Button, Callout, ConfirmInput, Dialog, DiffView, Drawer, Skeleton, Tone,
};
use leptos::prelude::*;

use crate::edit::{Outcome, missing_confirms, severity_tone};
use crate::screens::activity::diff_lines;
use crate::store::store;

#[component]
pub fn PendingBar() -> impl IntoView {
    let s = store();
    let e = s.edit;
    let dirty = Memo::new(move |_| e.dirty());
    let summary = move || {
        let n = e.pending();
        let errors = e.local_errors() + e.server_errors();
        let base = format!("{} pending change{}", n, if n == 1 { "" } else { "s" });
        if e.checking.get() {
            format!("{base} · checking…")
        } else if errors > 0 {
            format!(
                "{base} · {errors} error{}",
                if errors == 1 { "" } else { "s" }
            )
        } else {
            base
        }
    };
    view! {
        <Show when=move || dirty.get()>
            <div class="flex items-center gap-2" data-testid="pending-bar">
                <span class="text-sm whitespace-nowrap" role="status" aria-live="polite">{summary}</span>
                <Button label="Discard" small=true testid="discard" on_click=move |_| e.discard()/>
                <Button
                    label="Review"
                    variant="primary"
                    small=true
                    testid="review-open"
                    disabled=Signal::derive(move || e.local_errors() + e.server_errors() > 0)
                    reason=Signal::derive(move || {
                        let n = e.local_errors() + e.server_errors();
                        if n > 0 { format!("{n} error{} to fix first", if n == 1 { "" } else { "s" }) } else { String::new() }
                    })
                    on_click=move |_| e.open_review()
                />
            </div>
        </Show>
    }
}

#[component]
pub fn ReviewDrawer() -> impl IntoView {
    let s = store();
    let e = s.edit;
    let open = RwSignal::new(false);
    Effect::new(move |_| open.set(e.review.with(Option::is_some)));
    Effect::new(move |_| {
        if !open.get() && e.review.with_untracked(Option::is_some) {
            e.review.set(None);
        }
    });
    let title = Signal::derive(move || {
        e.review
            .with(|r| r.as_ref().map(|r| r.title.clone()))
            .unwrap_or_default()
    });
    let body = move || {
        let Some(plan) = e.review_plan() else {
            return view! {
                <div data-testid="review-loading">
                    <p class="mb-2 text-text-muted">"Checking your change on the server…"</p>
                    <Skeleton lines=4/>
                </div>
            }
            .into_any();
        };
        let changes = plan
            .changes
            .clone()
            .into_iter()
            .map(|c| {
                let sign = match c.kind {
                    ChangeKind::Add => "+",
                    ChangeKind::Remove => "−",
                    ChangeKind::Rename => "→",
                    ChangeKind::Set | ChangeKind::Other => "~",
                };
                view! {
                    <li class="flex gap-2" data-change=c.path.join("/")>
                        <span class="w-4 font-mono text-text-muted" aria-hidden="true">{sign}</span>
                        <span>{c.summary.clone()}</span>
                    </li>
                }
            })
            .collect_view();
        let refusals = plan
            .refusals
            .clone()
            .into_iter()
            .map(|r| {
                view! {
                    <div data-refusal=r.code.clone()>
                        <Callout tone=Tone::Danger title="This change would lock you out">
                            <p>{r.message.clone()}</p>
                        </Callout>
                    </div>
                }
            })
            .collect_view();
        let impacts = plan
            .impacts
            .clone()
            .into_iter()
            .map(|i| {
                let tone = match i.level {
                    Level::Danger => Tone::Danger,
                    Level::Warning => Tone::Warning,
                    Level::Info => Tone::Info,
                };
                let confirm = i.confirm.clone().map(|token| {
                    let t2 = token.clone();
                    let t3 = token.clone();
                    view! {
                        <ConfirmInput
                            token=token
                            value=Signal::derive(move || e.confirms.with(|c| c.get(&t2).cloned().unwrap_or_default()))
                            on_input=move |v: String| e.confirms.update(|c| { c.insert(t3.clone(), v); })
                        />
                    }
                });
                view! {
                    <div data-impact=i.code.clone() data-level=format!("{:?}", i.level).to_lowercase()>
                        <Callout tone=tone title=i.message.clone()>{confirm}</Callout>
                    </div>
                }
            })
            .collect_view();
        let diags = plan
            .diags
            .clone()
            .into_iter()
            .map(|d| {
                let where_ = d.path.as_ref().map(|p| p.join(".")).unwrap_or_default();
                view! {
                    <li class=format!("rounded-input border px-3 py-2 text-sm {}", severity_tone(&d).soft()) data-diag=d.code.clone()>
                        <strong>{where_}</strong>" "{d.message.clone()}
                    </li>
                }
            })
            .collect_view();
        let applies = plan
            .applies
            .iter()
            .filter(|a| a.how == burrow_admin_proto::schema::Applies::Restart)
            .map(|a| a.field.clone())
            .collect::<Vec<_>>();
        let restart = (!applies.is_empty()).then(|| {
            view! {
                <Callout tone=Tone::Info title="Takes effect after a restart">
                    <p>{format!("The server keeps its current {} until it restarts.", applies.join(", "))}</p>
                </Callout>
            }
        });
        let lines = diff_lines(&plan.diff);
        let empty_diff = lines.is_empty();
        let can = plan.can_apply();
        let errors = plan.errors();
        let p2 = plan.clone();
        let missing = Signal::derive(move || e.confirms.with(|c| missing_confirms(&p2, c)));
        let reason = Signal::derive(move || {
            if !can {
                if errors > 0 {
                    format!(
                        "{errors} error{} to fix first",
                        if errors == 1 { "" } else { "s" }
                    )
                } else {
                    "this change is refused".to_string()
                }
            } else if !missing.get().is_empty() {
                format!("type {} to confirm", missing.get().join(", "))
            } else {
                String::new()
            }
        });
        view! {
            <div class="space-y-4" data-testid="review" data-valid=plan.valid.to_string()>
                {refusals}
                {impacts}
                {restart}
                <section>
                    <h3 class="mb-1 font-semibold">"Changes"</h3>
                    <ul class="space-y-1" data-testid="review-changes">{changes}</ul>
                </section>
                {(!plan.diags.is_empty()).then(|| view! {
                    <section>
                        <h3 class="mb-1 font-semibold">"Problems found by the server"</h3>
                        <ul class="space-y-1" data-testid="review-diags">{diags}</ul>
                    </section>
                })}
                <section>
                    <h3 class="mb-1 font-semibold">"server.yaml"</h3>
                    {if empty_diff {
                        view! { <p class="text-text-muted">"The file does not change."</p> }.into_any()
                    } else {
                        view! { <DiffView lines=lines testid="review-diff"/> }.into_any()
                    }}
                </section>
                <div class="sticky bottom-0 flex items-center justify-end gap-2 border-t border-border bg-surface py-3">
                    <span class="flex-1 text-sm text-text-muted" data-testid="apply-reason">{move || reason.get()}</span>
                    <Button label="Keep editing" on_click=move |_| e.review.set(None)/>
                    {can.then(|| view! {
                        <Button
                            label="Apply"
                            variant="primary"
                            testid="apply"
                            busy=e.applying
                            disabled=Signal::derive(move || !missing.get().is_empty())
                            reason=reason
                            on_click=move |_| e.apply(s)
                        />
                    })}
                </div>
            </div>
        }
        .into_any()
    };
    view! {
        <Drawer open=open title=title testid="review-drawer">
            {body}
        </Drawer>
    }
}

#[component]
pub fn ConflictDialog() -> impl IntoView {
    let s = store();
    let e = s.edit;
    let open = RwSignal::new(false);
    Effect::new(move |_| open.set(e.conflict.with(Option::is_some)));
    let theirs = move || {
        e.conflict.with(|c| {
            c.as_ref().map(|c| {
                c.theirs
                    .iter()
                    .map(|ch| view! { <li>{ch.summary.clone()}</li> })
                    .collect_view()
            })
        })
    };
    view! {
        <Dialog open=open title="The config changed since you opened it" testid="conflict">
            <p class="mb-2">"Someone else, or a hand edit of server.yaml, changed the config while you were editing. Your changes are kept."</p>
            <ul class="mb-3 list-disc pl-5 text-sm" data-testid="conflict-theirs">{theirs}</ul>
            <div class="flex flex-wrap justify-end gap-2">
                <Button label="Discard my changes" testid="conflict-discard" on_click=move |_| {
                    e.conflict_revert.set(None);
                    e.discard();
                    crate::api::load_state(s);
                }/>
                <Button label="Check my changes against the new version" variant="primary" testid="conflict-recheck" on_click=move |_| {
                    e.conflict.set(None);
                    let revert = e.conflict_revert.get_untracked();
                    e.conflict_revert.set(None);
                    let st = s;
                    crate::api::request(burrow_admin_proto::msg::req::STATE, burrow_admin_proto::serde_json::json!({}), move |r| {
                        if let Ok(Ok(state)) = r.map(burrow_admin_proto::serde_json::from_value::<burrow_admin_proto::msg::State>) {
                            st.state.set(Some(std::sync::Arc::new(state)));
                            match revert.clone() {
                                Some(to) => st.edit.open_revert(st, to),
                                None => st.edit.rebase(st),
                            }
                        }
                    });
                }/>
            </div>
        </Dialog>
    }
}

#[component]
pub fn OutcomeBanner() -> impl IntoView {
    let e = store().edit;
    move || {
        e.outcome.get().map(|o| {
            let dismiss = move |_| e.outcome.set(None);
            match o {
                Outcome::Applied(a) => {
                    let restart = if a.restart_needed.is_empty() {
                        String::new()
                    } else {
                        format!(" Restart the server for: {}.", a.restart_needed.join(", "))
                    };
                    view! {
                        <div data-testid="outcome-applied">
                            <Banner tone=Tone::Success title="Applied" message=format!("server.yaml is now at {}.{restart}", a.rev)>
                                <Button label="Dismiss" small=true on_click=dismiss/>
                            </Banner>
                        </div>
                    }
                    .into_any()
                }
                Outcome::Refused(r) => {
                    let msg = r.iter().map(|x| x.message.clone()).collect::<Vec<_>>().join(" ");
                    view! {
                    <div data-testid="outcome-refused">
                        <Banner tone=Tone::Danger title="Refused: this change would lock you out" message=msg>
                            <Button label="Dismiss" small=true on_click=dismiss/>
                        </Banner>
                    </div>
                }
                .into_any()
                }
                Outcome::ReloadFailed(r) => view! {
                    <div data-testid="outcome-reload-failed">
                        <Banner
                            tone=Tone::Danger
                            title=if r.restored { "The server rejected the new config; the previous file was restored".to_string() } else { "The server rejected the new config".to_string() }
                            message=format!("{} {}", r.reason, r.hint)
                        >
                            <Button label="Dismiss" small=true on_click=dismiss/>
                        </Banner>
                    </div>
                }
                .into_any(),
                Outcome::Failed { code, message } => view! {
                    <div data-testid="outcome-failed" data-code=code.as_str()>
                        <Banner tone=Tone::Danger title="The change was not applied" message=message>
                            <Button label="Dismiss" small=true on_click=dismiss/>
                        </Banner>
                    </div>
                }
                .into_any(),
            }
        })
    }
}
