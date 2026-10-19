use burrow_admin_proto::msg::{self, AuditEntry, HistoryReply};
use burrow_admin_proto::plan::Diff;
use burrow_admin_proto::serde_json::{self, json};
use burrow_ui_kit::{
    Button, Callout, DiffLine, DiffView, EmptyState, Skeleton, Stat, Stats, Tone, tag_class,
};
use leptos::prelude::*;

use crate::api;
use crate::format;
use crate::route::{self, Link, Route};
use crate::screens::{DetailPane, ListPane, Row, master_detail, wide};
use crate::store::{short_rev, store};

pub fn diff_lines(d: &Diff) -> Vec<DiffLine> {
    let mut out = Vec::new();
    for h in &d.hunks {
        out.push(DiffLine {
            kind: '@',
            old: None,
            new: None,
            text: format!(
                "@@ -{},{} +{},{} @@",
                h.old_start, h.old_lines, h.new_start, h.new_lines
            ),
        });
        let (mut o, mut n) = (h.old_start, h.new_start);
        for (kind, text) in &h.lines {
            let k = kind.chars().next().unwrap_or(' ');
            let (old, new) = match k {
                '+' => (None, Some(n)),
                '-' => (Some(o), None),
                _ => (Some(o), Some(n)),
            };
            if k != '+' {
                o += 1;
            }
            if k != '-' {
                n += 1;
            }
            out.push(DiffLine {
                kind: if k == '+' || k == '-' { k } else { ' ' },
                old,
                new,
                text: text.clone(),
            });
        }
    }
    out
}

fn who(e: &AuditEntry) -> String {
    if e.visitor.is_empty() || e.visitor == "file" {
        "hand edit".to_string()
    } else {
        e.visitor.clone()
    }
}

fn hand(e: &AuditEntry) -> bool {
    e.visitor.is_empty() || e.visitor == "file"
}

fn rows_of(entries: &[AuditEntry], can_write: bool) -> Vec<Row> {
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut actions = vec!["View"];
            if e.revertible && can_write {
                actions.push("Revert");
            }
            let tone = if hand(e) {
                Tone::Warning
            } else {
                match e.changes.first().map(|c| c.kind) {
                    Some(burrow_admin_proto::plan::ChangeKind::Add) => Tone::Success,
                    Some(burrow_admin_proto::plan::ChangeKind::Remove) => Tone::Danger,
                    None => Tone::Neutral,
                    _ => Tone::Accent,
                }
            };
            Row {
                key: e.rev.clone(),
                to: Route::Activity(Some(e.rev.clone())),
                icon: "history",
                dot: Some(tone),
                title: e
                    .changes
                    .first()
                    .map(|c| c.summary.clone())
                    .unwrap_or_else(|| format!("No change ({})", e.result)),
                meta: short_rev(&e.rev),
                line2: format!("{} · {}", format::relative(e.at), who(e)),
                tags: if i == 0 {
                    vec![("current".to_string(), Tone::Success)]
                } else {
                    Vec::new()
                },
                filters: vec![if hand(e) { "hand" } else { "admins" }],
                actions: if i == 0 { Vec::new() } else { actions },
                ..Row::default()
            }
        })
        .collect()
}

#[component]
fn Version(entry: AuditEntry, newer: Vec<AuditEntry>) -> impl IntoView {
    let s = store();
    let diff: RwSignal<Option<Result<Vec<DiffLine>, String>>> = RwSignal::new(None);
    if entry.details_kept {
        api::request(
            msg::req::HISTORY,
            json!({ "limit": 1, "diff": entry.rev.clone() }),
            move |r| {
                let lines = r
                    .map_err(|e| e.message)
                    .and_then(|v| {
                        serde_json::from_value::<HistoryReply>(v).map_err(|e| e.to_string())
                    })
                    .and_then(|h| {
                        h.diff.map(|d| diff_lines(&d)).ok_or_else(|| {
                            "The details of this change are no longer kept.".to_string()
                        })
                    });
                let _ = diff.try_set(Some(lines));
            },
        );
    } else {
        diff.set(Some(Err(
            "The details of this change are no longer kept.".into()
        )));
    }
    let can_revert = entry.revertible && s.can_write_now();
    let to = entry.base.clone();
    let from = if entry.from.is_empty() {
        String::new()
    } else {
        format!(" from {}", entry.from)
    };
    let first = entry
        .changes
        .first()
        .map(|c| c.summary.clone())
        .unwrap_or_else(|| "No change".into());
    let first_path = entry
        .changes
        .first()
        .map(|c| c.path.join("."))
        .unwrap_or_default();
    let n = entry.changes.len();
    let written = if hand(&entry) {
        "written directly to server.yaml".to_string()
    } else {
        format!("by {}{from}", who(&entry))
    };
    let newer_list = newer
        .into_iter()
        .map(|n| {
            let rev = short_rev(&n.rev);
            view! {
                <li class="grid grid-cols-[70px_minmax(0,1fr)_auto] items-center gap-4 border-b border-rowline py-2.5 text-[13.5px] last:border-b-0">
                    <Link to=Route::Activity(Some(n.rev.clone())) class="font-mono text-[13px] text-text-muted no-underline hover:underline">{rev}</Link>
                    <span class="truncate">{n.changes.first().map(|c| c.summary.clone()).unwrap_or_default()}</span>
                    <span class="text-text-muted">{format!("{} · {}", who(&n), format::relative(n.at))}</span>
                </li>
            }
        })
        .collect::<Vec<_>>();
    let base_short = short_rev(&entry.base);
    let plan_note = format!(
        "It builds a plan from rev {base_short}, then goes through validation, the lockout guard, the impact summary and your confirmation, like any edit. The review then shows the impact summary from the server."
    );
    let label = "mb-2 text-[11px] font-bold tracking-[0.07em] text-text-faint uppercase";
    view! {
        <nav class="mb-1.5 flex gap-1.5 text-sm text-text-muted" aria-label="Breadcrumb">
            <Link to=Route::Activity(None) class="text-text-muted no-underline hover:text-text">"Activity"</Link>
            <span aria-hidden="true">"/"</span>
            <span class="font-semibold text-text">{format!("rev {}", short_rev(&entry.rev))}</span>
        </nav>
        <header class="mb-[22px] flex flex-wrap items-start gap-3.5">
            <div class="min-w-0 flex-1">
                <h1 class="flex items-center gap-2.5 text-[25px] font-bold tracking-[-0.015em]">
                    {format!("rev {}", short_rev(&entry.rev))}
                    <span class=tag_class(if hand(&entry) { Tone::Warning } else { Tone::Accent })>{if hand(&entry) { "hand edit".to_string() } else { who(&entry) }}</span>
                </h1>
                <p class="mt-1 text-base text-text-muted">{format!("{first} · {} · {written}", format::relative(entry.at))}</p>
            </div>
            {can_revert.then(|| view! {
                <Button label="Revert to before this change" variant="tinted" icon="revert" testid="revert" on_click=move |_| {
                    s.edit.open_revert(s, to.clone());
                    route::go(Route::Review);
                }/>
            })}
        </header>
        <Stats testid="version-stats">
            <Stat label="Rev" value=short_rev(&entry.rev) detail="this version"/>
            <Stat label="Base" value=base_short.clone() detail="the version it replaced"/>
            <Stat label="Author" value=who(&entry) detail=if hand(&entry) { "no admin session".to_string() } else { entry.from.clone() }/>
            <Stat label="Changed" value=n.to_string() unit={ if n == 1 { "field" } else { "fields" } } detail=first_path/>
        </Stats>
        <h3 class=label>"Diff of this change"</h3>
        <div class="mb-6 overflow-hidden rounded-card border border-border bg-surface">
            {move || match diff.get() {
                None => view! { <div class="p-4"><Skeleton lines=3/></div> }.into_any(),
                Some(Ok(lines)) => view! { <DiffView lines=lines testid="activity-diff"/> }.into_any(),
                Some(Err(e)) => view! { <p class="px-4 py-3 text-text-muted">{e}</p> }.into_any(),
            }}
        </div>
        <div class="mb-6">
            <Callout tone=Tone::Info title="A revert is an ordinary change.">
                <p>{plan_note}</p>
            </Callout>
        </div>
        {(!newer_list.is_empty()).then(|| view! {
            <h3 class=label>"Changes since then"</h3>
            <ul data-testid="since">{newer_list}</ul>
        })}
    }
}

#[component]
pub fn Activity(selected: Option<String>) -> impl IntoView {
    let s = store();
    let rows = Memo::new(move |_| {
        s.history
            .get()
            .map(|h| rows_of(&h, s.can_write()))
            .unwrap_or_default()
    });
    if selected.is_none()
        && wide()
        && let Some(first) = rows.get_untracked().first()
    {
        route::replace(first.to.clone());
    }
    let count = Signal::derive(move || {
        let n = rows.with(Vec::len);
        format!("{n} version{}", if n == 1 { "" } else { "s" })
    });
    let on_action = Callback::new(move |(rev, action): (String, String)| {
        let entry = s.history.with_untracked(|h| {
            h.as_ref()
                .and_then(|h| h.iter().find(|e| e.rev == rev).cloned())
        });
        match (action.as_str(), entry) {
            ("revert", Some(e)) => {
                s.edit.open_revert(s, e.base.clone());
                route::go(Route::Review);
            }
            _ => route::go(Route::Activity(Some(rev))),
        }
    });
    let sel = selected.clone();
    let list = view! {
        <ListPane
            title="Activity"
            count=count
            search_label="Search history"
            filters=vec![("all", "All".into()), ("admins", "Admins".into()), ("hand", "Hand edits".into())]
            groups=Vec::new()
            rows=rows
            selected=Signal::stored(sel)
            empty=("No changes yet".to_string(), "Changes made through burrow admin and hand edits of server.yaml show up here.".to_string())
            on_action=on_action
            no_counts=true
            testid="activity"
        />
    }
    .into_any();
    let body = move || {
        match s.history.get() {
        None => view! { <Skeleton lines=4/> }.into_any(),
        Some(h) if h.is_empty() => view! {
            <EmptyState title="No changes yet" message="Changes made through burrow admin and hand edits of server.yaml show up here." testid="activity-empty"/>
        }
        .into_any(),
        Some(h) => {
            let at = selected.as_ref().and_then(|r| h.iter().position(|e| &e.rev == r));
            match at {
                Some(i) => view! { <Version entry=h[i].clone() newer=h[..i].to_vec()/> }.into_any(),
                None => view! { <EmptyState icon="history" title="Pick a version on the left" message="Each one shows what changed and can be reverted."/> }.into_any(),
            }
        }
    }
    };
    let detail = view! { <DetailPane back=Route::Activity(None)>{body}</DetailPane> }.into_any();
    master_detail(list, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::plan::Hunk;

    #[test]
    fn hunks_become_numbered_lines() {
        let d = Diff {
            unified: String::new(),
            hunks: vec![Hunk {
                old_start: 3,
                old_lines: 2,
                new_start: 3,
                new_lines: 2,
                lines: vec![
                    (" ".into(), "a".into()),
                    ("-".into(), "b".into()),
                    ("+".into(), "c".into()),
                ],
            }],
        };
        let l = diff_lines(&d);
        assert_eq!(l[0].kind, '@');
        assert_eq!((l[1].old, l[1].new), (Some(3), Some(3)));
        assert_eq!((l[2].kind, l[2].old, l[2].new), ('-', Some(4), None));
        assert_eq!((l[3].kind, l[3].old, l[3].new), ('+', None, Some(4)));
    }
}
