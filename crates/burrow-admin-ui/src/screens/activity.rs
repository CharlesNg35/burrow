use burrow_admin_proto::msg::{self, AuditEntry, HistoryReply};
use burrow_admin_proto::plan::Diff;
use burrow_admin_proto::serde_json::{self, json};
use burrow_ui_kit::{
    Badge, Button, Card, DiffLine, DiffView, EmptyState, PageHeader, Skeleton, Tone,
};
use leptos::prelude::*;

use crate::api;
use crate::format;
use crate::store::store;

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

#[component]
fn Entry(entry: AuditEntry) -> impl IntoView {
    let open = RwSignal::new(false);
    let diff: RwSignal<Option<Result<Vec<DiffLine>, String>>> = RwSignal::new(None);
    let rev = entry.rev.clone();
    let load = move |_| {
        if open.get_untracked() {
            open.set(false);
            return;
        }
        open.set(true);
        if diff.get_untracked().is_some() {
            return;
        }
        api::request(
            msg::req::HISTORY,
            json!({ "limit": 1, "diff": rev.clone() }),
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
                diff.set(Some(lines));
            },
        );
    };
    let who = if entry.visitor.is_empty() || entry.visitor == "file" {
        "a hand edit".to_string()
    } else {
        format!("visitor {}", entry.visitor)
    };
    let from = if entry.from.is_empty() {
        String::new()
    } else {
        format!(" from {}", entry.from)
    };
    let tone = if entry.result == "applied" || entry.result == "ok" {
        Tone::Success
    } else {
        Tone::Warning
    };
    let changes = entry
        .changes
        .iter()
        .map(|c| view! { <li>{c.summary.clone()}</li> })
        .collect_view();
    let can_diff = entry.details_kept;
    let s = crate::store::store();
    let can_revert = entry.revertible && s.can_write();
    let to = entry.base.clone();
    view! {
        <li class="py-3" data-rev=entry.rev.clone() data-testid="activity-entry">
            <div class="flex flex-wrap items-center gap-3">
                <span class="text-sm text-text-muted tabular-nums">{format::when(entry.at)}</span>
                <span class="flex-1">{format!("{who}{from}")}</span>
                <Badge text=entry.result.clone() tone=tone/>
                <span class="font-mono text-xs text-text-muted">{entry.rev.clone()}</span>
            </div>
            <ul class="my-2 list-disc pl-5 text-sm">{changes}</ul>
            <div class="mb-2 flex gap-2">
                <Button
                    label=Signal::derive(move || if open.get() { "Hide diff".to_string() } else { "View diff".to_string() })
                    small=true
                    disabled=!can_diff
                    reason=if can_diff { String::new() } else { "The details of this change are no longer kept".to_string() }
                    testid="view-diff"
                    on_click=load
                />
                {can_revert.then(|| view! {
                    <Button
                        label="Revert to before this change"
                        small=true
                        testid="revert"
                        on_click=move |_| s.edit.open_revert(s, to.clone())
                    />
                })}
            </div>
            <Show when=move || open.get()>
                {move || match diff.get() {
                    None => view! { <Skeleton lines=3/> }.into_any(),
                    Some(Ok(lines)) => view! { <DiffView lines=lines testid="activity-diff"/> }.into_any(),
                    Some(Err(e)) => view! { <p class="text-text-muted">{e}</p> }.into_any(),
                }}
            </Show>
        </li>
    }
}

#[component]
pub fn Activity() -> impl IntoView {
    let s = store();
    let body = move || {
        match s.history.get() {
        None => view! { <Skeleton lines=4/> }.into_any(),
        Some(entries) if entries.is_empty() => view! {
            <EmptyState title="No changes yet" message="Changes made through burrow admin and hand edits of server.yaml show up here." testid="activity-empty"/>
        }
        .into_any(),
        Some(entries) => view! {
            <ul class="divide-y divide-border" data-testid="activity">
                {entries.into_iter().map(|e| view! { <Entry entry=e/> }).collect_view()}
            </ul>
        }
        .into_any(),
    }
    };
    view! {
        <PageHeader title="Activity" subtitle="The last changes to server.yaml, newest first"/>
        <Card>{body}</Card>
    }
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
