use burrow_admin_logic::grant::IN_THE_FILE;
use burrow_admin_proto::diag::Diag;
use burrow_admin_proto::plan::{Plan, Refusal};
use burrow_admin_proto::value::get;
use burrow_syntax::grant::is_grant_code;
use burrow_ui_kit::DiffLine;
use burrow_ui_kit::{
    Button, Callout, ConfirmInput, DiffView, Icon, PathDiagram, Segmented, Tone, button_class,
    tag_class,
};
use leptos::prelude::*;

use crate::edit::{Outcome, missing_confirms, severity_tone};
use crate::route::{self, Link, Route};
use crate::screens::activity::diff_lines;
use crate::store::store;

use super::guard::{Guard, Refused, footer_text, guard, lockout_fixes};
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
    let (added, removed) = if plan.files.is_empty() {
        diff_counts(&plan.diff)
    } else {
        plan.files
            .iter()
            .map(|f| diff_counts(&f.diff))
            .fold((0, 0), |(a, r), (x, y)| (a + x, r + y))
    };
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
    let impacts = impacts_view(&plan);
    let restart: Vec<String> = plan
        .applies
        .iter()
        .filter(|a| a.how == burrow_admin_proto::schema::Applies::Restart)
        .map(|a| a.field.clone())
        .collect();
    let confirms = confirms_view(&plan);
    let new_diags: Vec<_> = plan.diags.iter().filter(|d| !d.before).cloned().collect();
    let diags = diags_view(&new_diags);
    let reason = Signal::derive(move || {
        if refused {
            "This change is refused.".to_string()
        } else if !can {
            format!(
                "{} to fix first",
                burrow_admin_logic::wording::count_of(errors, "error")
            )
        } else if !missing.get().is_empty() {
            format!("Type {} to confirm", missing.get().join(", "))
        } else {
            String::new()
        }
    });
    let applies = burrow_admin_logic::wording::applies_label(if restart.is_empty() {
        &burrow_admin_proto::schema::Applies::Reload
    } else {
        &burrow_admin_proto::schema::Applies::Restart
    })
    .unwrap_or_default();
    let warnings = new_diags.iter().filter(|d| !d.is_error()).count();
    let label = "mb-2 text-[11px] font-bold tracking-[0.07em] text-text-faint uppercase";
    let facts = Facts {
        n,
        errors,
        warnings,
        empty,
        revert,
        refused,
        can,
        destructive,
        missing,
        reason,
    };
    view! {
        <div data-testid="review" data-valid=plan.valid.to_string()>
            <nav class="mb-1.5 text-sm font-semibold text-text" aria-label="Breadcrumb">"Review"</nav>
            {header_view(&facts, lockouts.is_empty(), applies)}
            {refused.then(|| view! { <Refused refusals=plan.refusals.clone()/> })}
            {paths.into_iter().map(|(name, p)| view! { <div data-review-path=name><PathDiagram data=p/></div> }).collect_view()}
            {plan.reformat.clone().map(|r| {
                let place = if r.scope.is_empty() {
                    "the whole file".to_string()
                } else {
                    burrow_config::label_of(&r.scope).unwrap_or_else(|| r.scope.join(" › "))
                };
                view! {
                    <div class="mb-6" data-testid="reformat">
                        <Callout tone=Tone::Warning title="Part of server.yaml is written fresh">
                            <p>{burrow_admin_logic::wording::rewritten(&place, &r.reason)}</p>
                        </Callout>
                    </div>
                }
            })}
            {impact_section(&plan, &restart, impacts, label)}
            {(!new_diags.is_empty()).then(|| view! {
                <h3 class=label>"Errors and warnings from this change"</h3>
                <ul class="mb-6 space-y-1" data-testid="review-diags">{diags}</ul>
            })}
            {(old_problems > 0).then(|| view! {
                <p class="-mt-3 mb-6 text-[12.5px] text-text-muted" data-testid="old-problems">
                    {format!("{} the config already had {} not shown here; ", burrow_admin_logic::wording::count_of(old_problems, "problem"), if old_problems == 1 { "is" } else { "are" })}
                    <Link to=Route::Overview class="text-accent-ink">"see the overview"</Link>
                    "."
                </p>
            })}
            {(!empty).then(|| diff_section(&plan, lines, added, removed, split))}
            {destructive.then(|| view! {
                <div class=format!("mb-6 rounded-[10px] p-3.5 {}", burrow_ui_kit::DANGER_CARD) data-testid="confirms">
                    <p class="flex items-center gap-2 text-sm font-semibold text-danger"><Icon name="alert"/>"Destructive change"</p>
                    {confirms}
                </div>
            })}
            {footer_view(&plan, &facts, guard_view)}
        </div>
    }
    .into_any()
}

struct Facts {
    n: usize,
    errors: usize,
    warnings: usize,
    empty: bool,
    revert: bool,
    refused: bool,
    can: bool,
    destructive: bool,
    missing: Signal<Vec<String>>,
    reason: Signal<String>,
}

fn impacts_view(plan: &Plan) -> impl IntoView + use<> {
    plan
        .impacts
        .iter()
        .filter(|i| i.code != burrow_admin_proto::codes::ADDS_CLIENT)
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
        .collect_view()
}

fn confirms_view(plan: &Plan) -> impl IntoView + use<> {
    let e = store().edit;
    plan
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
        .collect_view()
}

fn diags_view(new_diags: &[Diag]) -> impl IntoView + use<> {
    new_diags
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
                    <strong>{where_}</strong>" "{burrow_admin_logic::wording::diag_text(d)}
                </li>
            }
        })
        .collect_view()
}

fn header_view(f: &Facts, no_lockouts: bool, applies: &'static str) -> impl IntoView + use<> {
    let Facts {
        n,
        errors,
        warnings,
        empty,
        revert,
        refused,
        ..
    } = *f;
    view! {
    <header class="mb-[22px] flex flex-wrap items-start gap-3.5">
        <div class="min-w-0 flex-1">
            <h1 class="text-[25px] font-bold tracking-[-0.015em]">{if empty { "Nothing to apply".to_string() } else if revert { "Review the revert".to_string() } else { format!("Review {}", burrow_admin_logic::wording::count_of(n, "change")) }}</h1>
            <p class="mt-1 text-base text-text-muted" class:hidden=empty>
                {if refused {
                    format!(
                        "Refused by the {}",
                        if no_lockouts { "admin grant guard" } else { "lockout guard" }
                    )
                } else {
                    format!("Checked with the same rules as burrow validate · {applies}")
                }}
            </p>
        </div>
        {if refused || empty {
            ().into_any()
        } else if errors > 0 {
            view! { <span class=tag_class(Tone::Danger) data-testid="review-state">{format!("{} · {}", burrow_admin_logic::wording::count_of(errors, "error"), burrow_admin_logic::wording::count_of(warnings, "warning"))}</span> }.into_any()
        } else {
            view! { <span class=tag_class(Tone::Success) data-testid="review-state"><Icon name="check" small=true/>{format!("Valid · 0 errors · {}", burrow_admin_logic::wording::count_of(warnings, "warning"))}</span> }.into_any()
        }}
    </header>
    }
}

fn impact_section<V: IntoView + 'static>(
    plan: &Plan,
    restart: &[String],
    impacts: V,
    label: &'static str,
) -> impl IntoView + use<V> {
    let restart = restart.to_vec();
    (plan.impacts.iter().any(|i| i.code != burrow_admin_proto::codes::ADDS_CLIENT) || !restart.is_empty()).then(|| view! {
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
    })
}

fn diff_section(
    plan: &Plan,
    lines: Vec<DiffLine>,
    added: usize,
    removed: usize,
    split: RwSignal<bool>,
) -> impl IntoView + use<> {
    let secrets_in_diff = plan.secrets_in_diff.is_empty();
    view! {
    <div class="mb-2 flex items-center gap-2">
        <h3 class="text-[11px] font-bold tracking-[0.07em] text-text-faint uppercase">{if plan.files.is_empty() { burrow_admin_logic::wording::CHANGED_LINES } else { burrow_admin_logic::wording::CHANGED_FILES }}</h3>
        <span class="text-xs text-text-muted">{burrow_admin_logic::wording::diff_head(added, removed, !secrets_in_diff)}</span>
        <span class="ml-auto"></span>
        <Segmented
            name="diff-mode"
            label="Show the changed lines"
            options=vec![("unified".to_string(), "One column".to_string(), false), ("split".to_string(), "Side by side".to_string(), false)]
            value=Signal::derive(move || if split.get() { "split".to_string() } else { "unified".to_string() })
            on_change=Callback::new(move |v: String| split.set(v == "split"))
        />
    </div>
    {if plan.files.is_empty() {
        view! {
            <section class="mb-6 overflow-hidden rounded-card border border-border bg-surface">
                {if lines.is_empty() {
                    view! { <p class="px-4 py-3 text-text-muted">"The file does not change."</p> }.into_any()
                } else {
                    view! { <DiffView lines=lines split=split testid="review-diff"/> }.into_any()
                }}
            </section>
        }
        .into_any()
    } else {
        view! { <div class="mb-6" data-testid="file-diffs">{super::files::file_diffs(&plan.files, split, true)}</div> }.into_any()
    }}
    }
}

fn footer_view(plan: &Plan, f: &Facts, guard_view: Option<Guard>) -> impl IntoView + use<> {
    let s = store();
    let e = s.edit;
    let Facts {
        n,
        empty,
        revert,
        refused,
        can,
        destructive,
        missing,
        reason,
        ..
    } = *f;
    let back = move || route::go(s.prev.get_untracked());
    let base = plan.base.clone();
    view! {
    <burrow_ui_kit::StickyBar flush=true class="max-lg:[&>span]:flex-1 max-lg:[&_button]:w-full [&_button]:whitespace-nowrap">
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
            format!("The {} stays in Activity; you can go back to it.", store().version(&base))
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
                    label=if revert { "Apply the revert".to_string() } else { format!("Apply {}", burrow_admin_logic::wording::count_of(n, "change")) }
                    variant=if destructive { "danger" } else { "primary" }
                    testid="apply"
                    busy=e.applying
                    disabled=Signal::derive(move || !missing.get().is_empty())
                    reason=reason
                    on_click=move |_| e.apply(s)
                />
            }.into_any()
        }}
    </burrow_ui_kit::StickyBar>
    }
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
