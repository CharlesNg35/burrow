use burrow_admin_logic::grant::IN_THE_FILE;
use burrow_admin_proto::plan::{Plan, Refusal};
use burrow_admin_proto::value::get;
use burrow_syntax::grant::is_grant_code;
use burrow_ui_kit::{
    Button, Callout, ConfirmInput, DiffView, Icon, PathDiagram, Segmented, Tone, button_class,
    tag_class,
};
use leptos::prelude::*;

use crate::edit::{Outcome, missing_confirms, severity_tone};
use crate::route::{self, Link, Route};
use crate::screens::activity::diff_lines;
use crate::store::{short_rev, store};

use super::guard::{Refused, footer_text, guard, lockout_fixes};
use super::{change_groups, diff_counts, impact_icon, nothing_to_apply, place_of, service_paths};

pub(super) fn detail(plan: std::sync::Arc<Plan>, revert: bool) -> AnyView {
    let s = store();
    let e = s.edit;
    let n = change_groups(&plan).len();
    let errors = plan.errors();
    let empty = nothing_to_apply(&plan);
    let old_problems = plan.diags.iter().filter(|d| d.before).count();
    let paths = if revert {
        Vec::new()
    } else {
        let status = s.status.get_untracked();
        e.draft.with_untracked(|d| {
            service_paths(
                d.base(),
                d.draft(),
                status.as_deref(),
                &crate::domain::reserved_clients(),
            )
        })
    };
    let lines = diff_lines(&plan.diff);
    let (added, removed) = diff_counts(&plan.diff);
    let split = RwSignal::new(false);
    let p2 = plan.clone();
    let missing = Signal::derive(move || e.confirms.with(|c| missing_confirms(&p2, c)));
    let destructive = !plan.confirms().is_empty();
    let can = plan.can_apply();
    let refused = !plan.refusals.is_empty();
    let lockouts: Vec<Refusal> = plan
        .refusals
        .iter()
        .filter(|r| !is_grant_code(&r.code))
        .cloned()
        .collect();
    let guard_view = (!lockouts.is_empty()).then(|| guard(&lockouts));
    let impacts = plan
        .impacts
        .iter()
        .filter(|i| i.code != "adds-client")
        .map(|i| {
            let (icon, tone) = impact_icon(i.level);
            let (head, tail) = match i.message.split_once(" · ") {
                Some((a, b)) => (a.to_string(), b.to_string()),
                None => (i.message.clone(), String::new()),
            };
            let border = match tone {
                Tone::Danger => "border-danger/45",
                Tone::Warning => "border-warning/45",
                _ => "border-border",
            };
            view! {
                <div class=format!("flex items-start gap-3 rounded-card border bg-surface p-3.5 {border}") data-impact=i.code.clone() data-level=format!("{:?}", i.level).to_lowercase()>
                    <span class=format!("flex size-[26px] shrink-0 items-center justify-center rounded-md {}", tone.soft())><Icon name=icon small=true/></span>
                    <span class="min-w-0">
                        <span class="block text-sm font-semibold">{head}</span>
                        <span class="block text-[12.5px] text-text-muted">{tail}</span>
                    </span>
                </div>
            }
        })
        .collect_view();
    let restart: Vec<String> = plan
        .applies
        .iter()
        .filter(|a| a.how == burrow_admin_proto::schema::Applies::Restart)
        .map(|a| a.field.clone())
        .collect();
    let confirms = plan
        .impacts
        .iter()
        .filter_map(|i| i.confirm.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|token| {
            let (t2, t3) = (token.clone(), token.clone());
            view! {
                <ConfirmInput
                    token=token
                    value=Signal::derive(move || e.confirms.with(|c| c.get(&t2).cloned().unwrap_or_default()))
                    on_input=move |v: String| e.confirms.update(|c| { c.insert(t3.clone(), v); })
                />
            }
        })
        .collect_view();
    let new_diags: Vec<_> = plan.diags.iter().filter(|d| !d.before).cloned().collect();
    let diags = new_diags
        .iter()
        .map(|d| {
            let where_ = d
                .path
                .as_ref()
                .map(|p| {
                    store()
                        .schema
                        .with_untracked(|sc| sc.as_ref().map(|sc| place_of(&sc.root, p)))
                        .unwrap_or_else(|| p.join("."))
                })
                .unwrap_or_default();
            view! {
                <li class=format!("rounded-input border px-3 py-2 text-sm {}", severity_tone(d).soft()) data-diag=d.code.clone()>
                    <strong>{where_}</strong>" "{d.message.clone()}
                </li>
            }
        })
        .collect_view();
    let reason = Signal::derive(move || {
        if refused {
            "This change is refused.".to_string()
        } else if !can {
            format!(
                "{errors} error{} to fix first",
                if errors == 1 { "" } else { "s" }
            )
        } else if !missing.get().is_empty() {
            format!("Type {} to confirm", missing.get().join(", "))
        } else {
            String::new()
        }
    });
    let back = move || route::go(s.prev.get_untracked());
    let applies = if restart.is_empty() {
        "applies on reload"
    } else {
        "needs a restart"
    };
    let warnings = new_diags.iter().filter(|d| !d.is_error()).count();
    let label = "mb-2 text-[11px] font-bold tracking-[0.07em] text-text-faint uppercase";
    view! {
        <div data-testid="review" data-valid=plan.valid.to_string()>
            <nav class="mb-1.5 text-sm font-semibold text-text" aria-label="Breadcrumb">"Review"</nav>
            <header class="mb-[22px] flex flex-wrap items-start gap-3.5">
                <div class="min-w-0 flex-1">
                    <h1 class="text-[25px] font-bold tracking-[-0.015em]">{if empty { "Nothing to apply".to_string() } else if revert { "Review the revert".to_string() } else { format!("Review {n} change{}", if n == 1 { "" } else { "s" }) }}</h1>
                    <p class="mt-1 text-base text-text-muted" class:hidden=empty>
                        {if refused {
                            format!(
                                "Refused by the {}",
                                if lockouts.is_empty() { "admin grant guard" } else { "lockout guard" }
                            )
                        } else {
                            format!("Checked with the same rules as burrow validate · {applies}")
                        }}
                    </p>
                </div>
                {if refused || empty {
                    ().into_any()
                } else if errors > 0 {
                    view! { <span class=tag_class(Tone::Danger) data-testid="review-state">{format!("{errors} error{} · {warnings} warning{}", if errors == 1 { "" } else { "s" }, if warnings == 1 { "" } else { "s" })}</span> }.into_any()
                } else {
                    view! { <span class=tag_class(Tone::Success) data-testid="review-state"><Icon name="check" small=true/>{format!("Valid · 0 errors · {warnings} warning{}", if warnings == 1 { "" } else { "s" })}</span> }.into_any()
                }}
            </header>
            {refused.then(|| view! { <Refused refusals=plan.refusals.clone()/> })}
            {paths.into_iter().map(|(name, p)| view! { <div data-review-path=name><PathDiagram data=p/></div> }).collect_view()}
            {plan.reformat.clone().map(|r| {
                let place = if r.scope.is_empty() { "The whole file".to_string() } else { r.scope.join(".") };
                view! {
                    <div class="mb-6" data-testid="reformat">
                        <Callout tone=Tone::Warning title="Part of server.yaml is written fresh">
                            <p>{format!("{place} is rewritten: {}. Comments and layout there are not kept.", r.reason)}</p>
                        </Callout>
                    </div>
                }
            })}
            {(plan.impacts.iter().any(|i| i.code != "adds-client") || !restart.is_empty()).then(|| view! {
                <h3 class=label>"Impact"</h3>
                <div class="mb-6 grid gap-3 md:grid-cols-2" data-testid="impacts">
                    {impacts}
                    {(!restart.is_empty()).then(|| view! {
                        <div class="flex items-start gap-3 rounded-card border border-border bg-surface p-3.5" data-impact="restart">
                            <span class=format!("flex size-[26px] shrink-0 items-center justify-center rounded-md {}", Tone::Info.soft())><Icon name="info" small=true/></span>
                            <span class="min-w-0">
                                <span class="block text-sm font-semibold">"Takes effect after a restart"</span>
                                <span class="block text-[12.5px] text-text-muted">{format!("The server keeps its current {} until it restarts.", restart.join(", "))}</span>
                            </span>
                        </div>
                    })}
                </div>
            })}
            {(!new_diags.is_empty()).then(|| view! {
                <h3 class=label>"Errors and warnings from this change"</h3>
                <ul class="mb-6 space-y-1" data-testid="review-diags">{diags}</ul>
            })}
            {(old_problems > 0).then(|| view! {
                <p class="-mt-3 mb-6 text-[12.5px] text-text-muted" data-testid="old-problems">
                    {format!("{old_problems} problem{} the config already had {} not shown here; ", if old_problems == 1 { "" } else { "s" }, if old_problems == 1 { "is" } else { "are" })}
                    <Link to=Route::Overview class="text-accent-ink">"see the overview"</Link>
                    "."
                </p>
            })}
            {(!empty).then(|| view! {
            <div class="mb-2 flex items-center gap-2">
                <h3 class="text-[11px] font-bold tracking-[0.07em] text-text-faint uppercase">"server.yaml"</h3>
                <span class="text-xs text-text-muted">{format!("+{added} −{removed}{}", if plan.secrets_in_diff.is_empty() { "" } else { " · secrets masked" })}</span>
                <span class="ml-auto"></span>
                <Segmented
                    name="diff-mode"
                    label="Diff layout"
                    options=vec![("unified".to_string(), "Unified".to_string(), false), ("split".to_string(), "Split".to_string(), false)]
                    value=Signal::derive(move || if split.get() { "split".to_string() } else { "unified".to_string() })
                    on_change=Callback::new(move |v: String| split.set(v == "split"))
                />
            </div>
            <section class="mb-6 overflow-hidden rounded-card border border-border bg-surface">
                {if lines.is_empty() {
                    view! { <p class="px-4 py-3 text-text-muted">"The file does not change."</p> }.into_any()
                } else {
                    view! { <DiffView lines=lines split=split testid="review-diff"/> }.into_any()
                }}
            </section>
            })}
            {destructive.then(|| view! {
                <div class="mb-6 rounded-[10px] border border-l-[3px] border-border border-l-danger bg-danger-weak/50 p-3.5 dark:bg-surface" data-testid="confirms">
                    <p class="flex items-center gap-2 text-sm font-semibold text-danger"><Icon name="alert"/>"Destructive change"</p>
                    {confirms}
                </div>
            })}
            <div class="sticky bottom-4 z-20 flex flex-wrap items-center gap-3 rounded-card border border-border bg-surface px-4 py-3 shadow-overlay max-lg:[&>span]:flex-1 max-lg:[&_button]:w-full [&_button]:whitespace-nowrap" data-sticky-bar="">
                <span class="min-w-0 flex-1 basis-60 text-sm text-text-muted" data-testid="review-footer">{if empty {
                    if revert {
                        "The file already matches that version.".to_string()
                    } else {
                        "Your edits put every value back as it was; the file would not change.".to_string()
                    }
                } else if let Some(g) = guard_view.clone() {
                    footer_text(&g)
                } else if refused {
                    format!("{IN_THE_FILE}.")
                } else {
                    format!("rev {} is kept in history; you can revert it from Activity.", short_rev(&plan.base))
                }}</span>
                <span class="sr-only" data-testid="apply-reason">{move || reason.get()}</span>
                {guard_view.clone().map(lockout_fixes)}
                {(!revert).then(|| view! {
                    <Button label="Discard all" variant="ghost" testid="discard" on_click=move |_| {
                        let to = match s.prev.get_untracked() {
                            Route::Detail(sec, name) => {
                                let name = e.draft.with_untracked(|d| d.renamed_from(&sec, &name)).unwrap_or(name);
                                let there = e.draft.with_untracked(|d| get(d.base(), &[sec.clone(), name.clone()]).is_some());
                                if there { Route::Detail(sec, name) } else { Route::List(sec) }
                            }
                            other => other,
                        };
                        e.review.set(None);
                        e.discard();
                        route::go(to);
                    }/>
                })}
                {if revert {
                    view! {
                        <Button label="Cancel" variant="secondary" testid="cancel-revert" on_click=move |_| {
                            e.review.set(None);
                            back();
                        }/>
                    }.into_any()
                } else {
                    view! {
                        <Button label="Back" variant="secondary" testid="keep-editing" on_click=move |_| {
                            e.review.set(None);
                            back();
                        }/>
                    }.into_any()
                }}
                {if empty {
                    ().into_any()
                } else if refused || !can {
                    view! {
                        <button type="button" class=button_class("danger-pending", false) disabled=true data-testid="apply">
                            {refused.then(|| view! { <Icon name="lock" small=true/> })}
                            {if refused { "Apply refused" } else { "Apply" }}
                        </button>
                    }.into_any()
                } else {
                    view! {
                        <Button
                            label=if revert { "Apply the revert".to_string() } else { format!("Apply {n} change{}", if n == 1 { "" } else { "s" }) }
                            variant=if destructive { "danger" } else { "primary" }
                            testid="apply"
                            busy=e.applying
                            disabled=Signal::derive(move || !missing.get().is_empty())
                            reason=reason
                            on_click=move |_| e.apply(s)
                        />
                    }.into_any()
                }}
            </div>
        </div>
    }
    .into_any()
}

pub(super) fn outcome_view() -> impl IntoView {
    let e = store().edit;
    move || {
        e.outcome.get().and_then(|o| match o {
            Outcome::Applied(_) => None,
            Outcome::Refused(r) => Some(view! { <Refused refusals=r/> }.into_any()),
            Outcome::ReloadFailed(r) => Some(
                view! {
                    <div class="mb-4" data-testid="outcome-reload-failed">
                        <Callout tone=Tone::Danger title=if r.restored { "The server rejected the new config; the previous file was restored".to_string() } else { "The server rejected the new config".to_string() }>
                            <p>{format!("{} {}", r.reason, r.hint)}</p>
                        </Callout>
                    </div>
                }
                .into_any(),
            ),
            Outcome::Unanswered => Some(
                view! {
                    <div class="mb-4" data-testid="outcome-unanswered">
                        <Callout tone=Tone::Warning title="The connection closed before the server answered"><p>"The change may have been applied. Check Activity once the connection is back."</p></Callout>
                    </div>
                }
                .into_any(),
            ),
            Outcome::Failed { code, message } => Some(
                view! {
                    <div class="mb-4" data-testid="outcome-failed" data-code=code.as_str()>
                        <Callout tone=Tone::Danger title="The change was not applied"><p>{message}</p></Callout>
                    </div>
                }
                .into_any(),
            ),
        })
    }
}
