use burrow_admin_logic::changeset::get;
use burrow_admin_proto::plan::{ChangeKind, Diff, Level, Plan, Refusal};
use burrow_admin_proto::serde_json::Value;
use burrow_ui_kit::{
    Button, Callout, ConfirmInput, Dialog, DiffView, Edge, EdgeState, EmptyState, Hop, HopState,
    Icon, PathData, PathDiagram, Segmented, Skeleton, Tone, button_class, tag_class,
};
use leptos::prelude::*;

use crate::edit::{Outcome, missing_confirms, severity_tone};
use crate::route::{self, Link, Route};
use crate::screens::activity::diff_lines;
use crate::screens::paths::{self, Problems};
use crate::screens::{DetailPane, ListPane, Row, master_detail};
use crate::store::{short_rev, store};

pub fn diff_counts(d: &Diff) -> (usize, usize) {
    let lines = d.hunks.iter().flat_map(|h| h.lines.iter());
    lines.fold((0, 0), |(a, r), (k, _)| match k.as_str() {
        "+" => (a + 1, r),
        "-" => (a, r + 1),
        _ => (a, r),
    })
}

fn kind_look(k: ChangeKind) -> (&'static str, Tone) {
    match k {
        ChangeKind::Add => ("plus", Tone::Success),
        ChangeKind::Remove => ("trash", Tone::Danger),
        ChangeKind::Rename => ("arrow", Tone::Accent),
        ChangeKind::Set | ChangeKind::Other => ("edit", Tone::Accent),
    }
}

fn impact_icon(l: Level) -> (&'static str, Tone) {
    match l {
        Level::Danger => ("alert-circle", Tone::Danger),
        Level::Warning => ("alert", Tone::Warning),
        Level::Info => ("info", Tone::Info),
    }
}

pub fn service_paths(base: &Value, draft: &Value) -> Vec<(String, PathData)> {
    let empty = serde_json_map();
    let b = base
        .get("services")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let d = draft
        .get("services")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let none = Problems {
        source: None,
        clients: None,
        ports: None,
    };
    let mut names: Vec<&String> = b.keys().chain(d.keys()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter_map(|n| {
            let (old, new) = (b.get(n), d.get(n));
            if old == new {
                return None;
            }
            let mark = |p: &mut PathData, hop: HopState, edge: EdgeState| {
                for h in &mut p.hops {
                    h.state = hop;
                    h.dot = None;
                }
                for e in &mut p.edges {
                    e.state = edge;
                }
            };
            let data = match (old, new) {
                (Some(o), None) => {
                    let mut p = paths::service(n, o, None, 0.0, &none, true);
                    mark(&mut p, HopState::Removed, EdgeState::Removed);
                    p.summary = format!("Service {n} is removed");
                    p
                }
                (None, Some(x)) => {
                    let mut p = paths::service(n, x, None, 0.0, &none, true);
                    mark(&mut p, HopState::Added, EdgeState::Added);
                    p.summary = format!("Service {n} is added");
                    p
                }
                (Some(o), Some(x)) => {
                    let before = paths::service(n, o, None, 0.0, &none, true);
                    let mut p = paths::service(n, x, None, 0.0, &none, true);
                    for (h, was) in p.hops.iter_mut().zip(before.hops.iter()) {
                        h.dot = None;
                        h.state = if h.value != was.value || h.unit != was.unit {
                            HopState::Added
                        } else {
                            HopState::Normal
                        };
                        if h.value != was.value {
                            h.badge = Some((format!("was {}", was.value), Tone::Neutral));
                        }
                    }
                    for e in &mut p.edges {
                        e.state = EdgeState::Live;
                    }
                    p.summary = format!("Service {n} changes");
                    p
                }
                (None, None) => return None,
            };
            Some((n.clone(), data))
        })
        .collect()
}

fn serde_json_map() -> burrow_admin_proto::serde_json::Map<String, Value> {
    burrow_admin_proto::serde_json::Map::new()
}

fn session_ip(from: &str) -> String {
    let host = match from.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !h.is_empty() => h,
        _ => from,
    };
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

#[derive(Clone)]
struct Guard {
    me: String,
    ip: String,
    code: String,
    path: Vec<String>,
    base: Vec<String>,
    now: Vec<String>,
}

fn list_at(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(crate::form::plain).collect())
        .unwrap_or_default()
}

fn guard(refusals: &[Refusal]) -> Guard {
    let s = store();
    let e = s.edit;
    let (me, from) = s.welcome.with_untracked(|w| {
        w.as_ref()
            .map(|w| (w.you.visitor.clone(), w.you.from.clone()))
            .unwrap_or_default()
    });
    let code = ["self-removed", "self-demoted", "self-source", "self-secret"]
        .into_iter()
        .find(|c| refusals.iter().any(|r| r.code == *c))
        .map(str::to_string)
        .or_else(|| refusals.first().map(|r| r.code.clone()))
        .unwrap_or_default();
    let mut path = vec!["visitors".to_string(), me.clone()];
    match code.as_str() {
        "self-demoted" => path.push("admin".into()),
        "self-source" => path.push("admin_from".into()),
        _ => {}
    }
    let base = e.draft.with_untracked(|d| list_at(get(d.base(), &path)));
    let now = list_at(e.peek(&path).as_ref());
    Guard {
        me,
        ip: session_ip(&from),
        code,
        path,
        base,
        now,
    }
}

fn lockout_path(g: &Guard) -> PathData {
    let session = if g.ip.is_empty() {
        "this session".to_string()
    } else {
        format!("this session · {}", g.ip)
    };
    let (above, below) = match g.code.as_str() {
        "self-removed" => ("removed".to_string(), String::new()),
        "self-demoted" => ("admin".to_string(), "not write".to_string()),
        "self-source" => (
            "admin_from".to_string(),
            if g.now.is_empty() {
                "empty".to_string()
            } else {
                g.now.join(", ")
            },
        ),
        "self-secret" => ("secret".to_string(), "removed".to_string()),
        other => (other.trim_start_matches("self-").to_string(), String::new()),
    };
    PathData {
        hops: vec![
            Hop {
                icon: "person",
                head: "Visitor".into(),
                value: g.me.clone(),
                mono: true,
                unit: session,
                state: HopState::Current,
                ..Hop::default()
            },
            Hop {
                icon: "box",
                head: "Server".into(),
                value: "this server".into(),
                dot: Some(Tone::Danger),
                unit: "would refuse this session".into(),
                state: HopState::Bad,
                ..Hop::default()
            },
        ],
        edges: vec![Edge {
            above,
            below,
            state: EdgeState::Error,
        }],
        summary: format!(
            "Visitor {} would lose its admin session on this server",
            g.me
        ),
    }
}

fn footer_text(g: &Guard) -> String {
    match g.code.as_str() {
        "self-source" if !g.base.is_empty() => format!(
            "Keep {} in admin_from, or apply from an allowed address.",
            g.base.join(", ")
        ),
        "self-source" => "Undo the admin_from change, or apply from an allowed address.".into(),
        _ => "Change it back to keep your admin access.".into(),
    }
}

#[component]
fn Lockout(refusals: Vec<Refusal>) -> impl IntoView {
    let g = guard(&refusals);
    let codes = refusals
        .iter()
        .map(|r| r.code.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let msg = refusals
        .iter()
        .map(|r| r.message.clone())
        .collect::<Vec<_>>()
        .join("; ");
    let mut chars = msg.chars();
    let msg = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect::<String>())
        .unwrap_or_default();
    view! {
        <PathDiagram data=lockout_path(&g) testid="lockout-path"/>
        <div class="mb-4" data-testid="lockout" data-refusal=g.code.clone()>
            <Callout tone=Tone::Danger title="This change would lock you out">
                <p>{format!("{msg}. The server refuses changes that remove the current admin session's access.")}</p>
                <p class="mt-1 font-mono text-xs">{format!("guard: {codes}")}</p>
            </Callout>
        </div>
    }
}

fn lockout_fixes(g: Guard) -> impl IntoView {
    let e = store().edit;
    let (p1, p2, ip2) = (g.path.clone(), g.path.clone(), g.ip.clone());
    let fixable = matches!(
        g.code.as_str(),
        "self-removed" | "self-demoted" | "self-source" | "self-secret"
    );
    let can_allow =
        g.code == "self-source" && !g.ip.is_empty() && e.peek(&g.path).is_none_or(|v| v.is_array());
    let keep = match g.code.as_str() {
        "self-removed" => format!("Keep visitor {}", g.me),
        "self-demoted" => "Keep write admin".to_string(),
        "self-source" if !g.base.is_empty() => format!("Keep {}", g.base.join(", ")),
        _ => "Undo this change".to_string(),
    };
    let ip = g.ip.clone();
    view! {
        {can_allow.then(|| view! {
            <button type="button" class=format!("{} whitespace-nowrap", button_class("tinted", false)) data-testid="fix-allow" on:click=move |_| {
                let mut list = e.peek(&p1).and_then(|v| v.as_array().cloned()).unwrap_or_default();
                list.push(Value::String(ip2.clone()));
                e.set(&p1, Some(Value::Array(list)));
                e.outcome.set(None);
                e.revalidate_now();
            }>
                <Icon name="plus" small=true/>
                {format!("Also allow {ip}")}
            </button>
        })}
        {fixable.then(|| view! {
            <button type="button" class=format!("{} whitespace-nowrap", button_class("tinted", false)) data-testid="fix-undo" on:click=move |_| {
                let base = e.draft.with_untracked(|d| get(d.base(), &p2).cloned());
                e.set(&p2, base);
                e.outcome.set(None);
                e.revalidate_now();
            }>
                <Icon name="revert" small=true/>
                {keep.clone()}
            </button>
        })}
    }
}

fn detail(plan: std::sync::Arc<Plan>, revert: bool) -> AnyView {
    let s = store();
    let e = s.edit;
    let n = plan.changes.len();
    let errors = plan.errors();
    let base_rev = short_rev(&plan.base);
    let paths = if revert {
        Vec::new()
    } else {
        e.draft
            .with_untracked(|d| service_paths(d.base(), d.draft()))
    };
    let lines = diff_lines(&plan.diff);
    let (added, removed) = diff_counts(&plan.diff);
    let split = RwSignal::new(false);
    let p2 = plan.clone();
    let missing = Signal::derive(move || e.confirms.with(|c| missing_confirms(&p2, c)));
    let destructive = !plan.confirms().is_empty();
    let can = plan.can_apply();
    let refused = !plan.refusals.is_empty();
    let guard_view = refused.then(|| guard(&plan.refusals));
    let impacts = plan
        .impacts
        .iter()
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
        .filter_map(|i| i.confirm.clone().map(|t| (t, i.message.clone())))
        .map(|(token, message)| {
            let (t2, t3) = (token.clone(), token.clone());
            view! {
                <ConfirmInput
                    token=token
                    message=message
                    value=Signal::derive(move || e.confirms.with(|c| c.get(&t2).cloned().unwrap_or_default()))
                    on_input=move |v: String| e.confirms.update(|c| { c.insert(t3.clone(), v); })
                />
            }
        })
        .collect_view();
    let diags = plan
        .diags
        .iter()
        .map(|d| {
            let where_ = d.path.as_ref().map(|p| p.join(".")).unwrap_or_default();
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
    let host = crate::app::host_line(s);
    let applies = if restart.is_empty() {
        "applies on reload"
    } else {
        "needs a restart"
    };
    let warnings = plan.diags.len() - errors;
    let destructive_text = plan
        .impacts
        .iter()
        .filter(|i| i.confirm.is_some())
        .map(|i| {
            i.message
                .split(" · ")
                .next()
                .unwrap_or_default()
                .to_lowercase()
        })
        .collect::<Vec<_>>()
        .join(" and ");
    let label = "mb-2 text-[11px] font-bold tracking-[0.07em] text-text-faint uppercase";
    view! {
        <div data-testid="review" data-valid=plan.valid.to_string()>
            <nav class="mb-1.5 text-sm font-semibold text-text" aria-label="Breadcrumb">"Review"</nav>
            <header class="mb-[22px] flex flex-wrap items-start gap-3.5">
                <div class="min-w-0 flex-1">
                    <h1 class="text-[25px] font-bold tracking-[-0.015em]">{if revert { "Review the revert".to_string() } else { format!("Review {n} change{}", if n == 1 { "" } else { "s" }) }}</h1>
                    <p class="mt-1 text-base text-text-muted">
                        {if refused {
                            format!("Validated against rev {base_rev} · refused by the lockout guard")
                        } else if host.is_empty() {
                            format!("Validated against rev {base_rev} · {applies}")
                        } else {
                            format!("Validated against rev {base_rev} on {host} · {applies}")
                        }}
                    </p>
                </div>
                {if refused {
                    ().into_any()
                } else if errors > 0 {
                    view! { <span class=tag_class(Tone::Danger) data-testid="review-state">{format!("{errors} error{} · {warnings} warning{}", if errors == 1 { "" } else { "s" }, if warnings == 1 { "" } else { "s" })}</span> }.into_any()
                } else {
                    view! { <span class=tag_class(Tone::Success) data-testid="review-state"><Icon name="check" small=true/>{format!("Valid · 0 errors · {warnings} warning{}", if warnings == 1 { "" } else { "s" })}</span> }.into_any()
                }}
            </header>
            {refused.then(|| view! { <Lockout refusals=plan.refusals.clone()/> })}
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
            {(!plan.impacts.is_empty() || !restart.is_empty()).then(|| view! {
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
            {(!plan.diags.is_empty()).then(|| view! {
                <h3 class=label>"Problems found by the server"</h3>
                <ul class="mb-6 space-y-1" data-testid="review-diags">{diags}</ul>
            })}
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
            {destructive.then(|| view! {
                <div class="mb-6 rounded-card border border-danger/45 bg-danger-weak p-4 text-co-danger" data-testid="confirms">
                    <p class="mb-2 flex items-center gap-2 font-semibold"><Icon name="alert"/>{format!("Destructive: {destructive_text}.")}</p>
                    <div class="space-y-3">{confirms}</div>
                </div>
            })}
            <div class="sticky bottom-4 z-20 flex flex-wrap items-center gap-3 rounded-card border border-border bg-surface px-4 py-3 shadow-overlay max-lg:[&>button]:flex-1 [&>button]:whitespace-nowrap" data-sticky-bar="">
                <span class="min-w-0 flex-1 basis-60 text-sm text-text-muted">{if let Some(g) = guard_view.clone() {
                    footer_text(&g)
                } else {
                    format!("rev {} is kept in history; you can revert it from Activity.", short_rev(&plan.base))
                }}</span>
                <span class="sr-only" data-testid="apply-reason">{move || reason.get()}</span>
                {guard_view.clone().map(lockout_fixes)}
                <Button label="Discard all" variant="ghost" testid="discard" on_click=move |_| {
                    e.review.set(None);
                    e.discard();
                    back();
                }/>
                <Button label="Keep editing" variant="secondary" testid="keep-editing" on_click=move |_| {
                    e.review.set(None);
                    back();
                }/>
                {if refused || !can {
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

fn outcome_view() -> impl IntoView {
    let e = store().edit;
    move || {
        e.outcome.get().and_then(|o| match o {
            Outcome::Applied(_) => None,
            Outcome::Refused(r) => Some(view! { <Lockout refusals=r/> }.into_any()),
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

#[component]
pub fn ReviewPage() -> impl IntoView {
    let s = store();
    let e = s.edit;
    if e.review.with_untracked(Option::is_none) && untrack(|| e.dirty()) {
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
                None => route::go(s.prev.get_untracked()),
            }
        }
        rev
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
        let mut out: Vec<Row> = p
            .changes
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (icon, tone) = kind_look(c.kind);
                Row {
                    key: format!("{i}"),
                    to: Route::Review,
                    icon,
                    dot: Some(tone),
                    title: c.summary.clone(),
                    line2: c.path.join("."),
                    group: "Pending changes".into(),
                    filters: vec!["changes"],
                    ..Row::default()
                }
            })
            .collect();
        for (i, imp) in p.impacts.iter().enumerate() {
            let (icon, tone) = impact_icon(imp.level);
            let (head, tail) = match imp.message.split_once(" · ") {
                Some((a, b)) => (a.to_string(), b.to_string()),
                None => (imp.message.clone(), String::new()),
            };
            out.push(Row {
                key: format!("impact-{i}"),
                to: Route::Review,
                icon,
                dot: Some(tone),
                title: head,
                line2: tail,
                group: "Impact".into(),
                filters: vec!["changes"],
                ..Row::default()
            });
        }
        for (i, d) in p.diags.iter().enumerate() {
            out.push(Row {
                key: format!("diag-{i}"),
                to: Route::Review,
                icon: if d.is_error() {
                    "alert-circle"
                } else {
                    "alert"
                },
                dot: Some(if d.is_error() {
                    Tone::Danger
                } else {
                    Tone::Warning
                }),
                title: d.message.clone(),
                line2: d.path.as_ref().map(|p| p.join(".")).unwrap_or_default(),
                group: "Diagnostics".into(),
                filters: vec!["diagnostics"],
                ..Row::default()
            });
        }
        out
    });
    let count = Signal::derive(move || {
        plan.with(|p| {
            p.as_ref()
                .map(|p| format!("rev {}", short_rev(&p.base)))
                .unwrap_or_default()
        })
    });
    let n_diags =
        Signal::derive(move || plan.with(|p| p.as_ref().map(|p| p.diags.len()).unwrap_or(0)));
    let list = view! {
        <ListPane
            title="Review"
            count=count
            search_label="Search changes"
            filters=vec![("changes", "Changes".into()), ("diagnostics", format!("Diagnostics {}", n_diags.get_untracked()))]
            groups=vec!["Pending changes", "Impact", "Diagnostics"]
            no_counts=true
            default_filter="changes"
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

#[component]
pub fn ConflictDialog() -> impl IntoView {
    let s = store();
    let e = s.edit;
    let open = RwSignal::new(false);
    Effect::new(move |_| open.set(e.conflict.with(Option::is_some)));
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
            <h3 class=label>{move || format!("Changed on the server · rev {}", current())}</h3>
            <ul class="mb-4 text-sm" data-testid="conflict-theirs">{theirs}</ul>
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
                    open.set(false);
                    e.conflict.set(None);
                    route::go(Route::Activity(rev));
                }/>
                <Button label=Signal::derive(move || format!("Review my draft on rev {}", current())) variant="primary" icon="refresh" testid="conflict-recheck" on_click=move |_| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::plan::Hunk;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn the_review_marks_removed_added_and_changed_services() {
        let base = json!({"services": {"ssh": {"client": "a", "bind": "0.0.0.0:2222", "target": "x:22"}, "web": {"client": "a", "bind": "0.0.0.0:443", "target": "x:80"}}});
        let draft = json!({"services": {"web": {"client": "a", "bind": "0.0.0.0:8443", "target": "x:80"}, "db": {"client": "a", "target": "y:5432"}}});
        let p = service_paths(&base, &draft);
        let names: Vec<&str> = p.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["db", "ssh", "web"]);
        assert!(p[0].1.hops.iter().all(|h| h.state == HopState::Added));
        assert!(p[1].1.hops.iter().all(|h| h.state == HopState::Removed));
        let web = &p[2].1;
        assert_eq!(web.hops[1].state, HopState::Added);
        assert_eq!(web.hops[1].badge.as_ref().unwrap().0, "was 0.0.0.0:443");
        assert_eq!(web.hops[3].state, HopState::Normal);
    }

    #[test]
    fn diff_counts_and_session_address() {
        let d = Diff {
            unified: String::new(),
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 2,
                new_start: 1,
                new_lines: 2,
                lines: vec![
                    ("+".into(), "a".into()),
                    ("-".into(), "b".into()),
                    ("-".into(), "c".into()),
                    (" ".into(), "d".into()),
                ],
            }],
        };
        assert_eq!(diff_counts(&d), (1, 2));
        assert_eq!(session_ip("203.0.113.7:51000"), "203.0.113.7");
        assert_eq!(session_ip("[2001:db8::1]:443"), "2001:db8::1");
        assert_eq!(session_ip("203.0.113.7"), "203.0.113.7");
    }
}
