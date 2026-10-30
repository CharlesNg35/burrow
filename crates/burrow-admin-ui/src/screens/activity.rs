use burrow_admin_proto::msg::{self, AuditEntry, HistoryReply};
use burrow_admin_proto::plan::Diff;
use burrow_admin_proto::serde_json;
use burrow_ui_kit::{
    Button, DiffLine, DiffView, EmptyState, PageHeader, Skeleton, Stat, Stats, Tone,
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

fn title_of(e: &AuditEntry, version: &dyn Fn(&str) -> String) -> String {
    if let Some(to) = &e.reverts {
        return format!("Reverted to the {}", version(to));
    }
    e.changes
        .iter()
        .find(|c| c.part_of.is_none())
        .or(e.changes.first())
        .map(|c| c.summary.clone())
        .unwrap_or_else(|| format!("No change ({})", e.result))
}

fn repeats(entries: &[AuditEntry], i: usize) -> Option<&AuditEntry> {
    let e = &entries[i];
    if e.reverts.is_some() {
        return None;
    }
    entries[i + 1..].iter().find(|o| o.rev == e.rev)
}

fn hand(e: &AuditEntry) -> bool {
    e.visitor.is_empty() || e.visitor == "file"
}

fn rows_of(entries: &[AuditEntry], can_write: bool, current: &str) -> Vec<Row> {
    let now = entries.iter().position(|e| e.rev == current);
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut actions = vec!["View"];
            if e.revertible && can_write && revert_target(e) != current {
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
                key: e.id.to_string(),
                to: Route::Activity(Some(e.id.to_string())),
                icon: "history",
                dot: Some(tone),
                title: title_of(e, &|r| store().version(r)),
                meta: short_rev(&e.rev),
                line2: {
                    let same = repeats(entries, i);
                    match same {
                        Some(o) => format!(
                            "{} · {} · same file as the version from {}",
                            format::relative(e.at),
                            who(e),
                            format::relative(o.at)
                        ),
                        None => format!("{} · {}", format::relative(e.at), who(e)),
                    }
                },
                tags: if Some(i) == now {
                    vec![("current".to_string(), Tone::Success)]
                } else {
                    Vec::new()
                },
                filters: vec![if hand(e) { "hand" } else { "admins" }],
                actions: if Some(i) == now { Vec::new() } else { actions },
                ..Row::default()
            }
        })
        .collect()
}

#[component]
fn Version(entry: AuditEntry, newer: Vec<AuditEntry>) -> impl IntoView {
    let s = store();
    let diff: RwSignal<Option<Result<Vec<DiffLine>, String>>> = RwSignal::new(None);
    let ids = burrow_admin_logic::history::has_ids(s.welcome.get_untracked().as_deref());
    let asked = burrow_admin_logic::history::diff_request(&entry, &newer, ids);
    if entry.details_kept
        && let Some(asked) = asked
    {
        api::request(msg::req::HISTORY, api::body(&asked), move |r| {
            let lines = r
                .map_err(|e| e.message)
                .and_then(|v| serde_json::from_value::<HistoryReply>(v).map_err(|e| e.to_string()))
                .and_then(|h| {
                    h.diff
                        .map(|d| diff_lines(&d))
                        .ok_or_else(|| "The details of this change are no longer kept.".to_string())
                });
            let _ = diff.try_set(Some(lines));
        });
    } else if entry.details_kept {
        diff.set(Some(Err(burrow_admin_logic::history::OLDER_REPEAT.into())));
    } else {
        diff.set(Some(Err(
            "The details of this change are no longer kept.".into()
        )));
    }
    let to = revert_target(&entry);
    let matches = {
        let to = to.clone();
        Signal::derive(move || {
            s.state
                .with(|st| st.as_ref().is_some_and(|st| st.rev == to))
        })
    };
    let can_revert = entry.revertible && s.can_write_now();
    let revert_label = if entry.base.is_empty() {
        "Revert to this version"
    } else {
        "Revert to before this change"
    };
    let title = title_of(&entry, &|r| store().version(r));
    let newer_list = newer
        .into_iter()
        .map(|n| {
            let rev = format::stamp(n.at as f64 * 1000.0);
            view! {
                <li class="grid grid-cols-[70px_minmax(0,1fr)_auto] items-center gap-4 border-b border-rowline py-2.5 text-[13.5px] last:border-b-0">
                    <Link to=Route::Activity(Some(n.id.to_string())) class="font-mono text-[13px] text-text-muted no-underline hover:underline">{rev}</Link>
                    <span class="truncate">{title_of(&n, &|r| store().version(r))}</span>
                    <span class="text-text-muted">{format!("{} · {}", who(&n), format::relative(n.at))}</span>
                </li>
            }
        })
        .collect::<Vec<_>>();
    let label = "mb-2 text-[11px] font-bold tracking-[0.07em] text-text-faint uppercase";
    view! {
        {crate::screens::crumbs(Route::Activity(None), "Activity".into(), store().version(&entry.rev), Signal::stored(false), None)}
        <PageHeader actions={view! { {can_revert.then(|| view! {
                <Button
                    label=revert_label
                    variant="tinted"
                    icon="revert"
                    testid="revert"
                    disabled=matches
                    reason=Signal::derive(move || if matches.get() { "The file already matches this version".to_string() } else { String::new() })
                    on_click=move |_| {
                        s.edit.open_revert(s, to.clone());
                        route::go(Route::Review);
                    }
                />
            })} }.into_any()}>{title}</PageHeader>
        <Stats testid="version-stats">
            <Stat label="Version" value=format::stamp(entry.at as f64 * 1000.0) detail=format!("replaced the {} · id {}", store().version(&entry.base), short_rev(&entry.rev))/>
            <Stat label="By" value=if hand(&entry) { "hand edit".to_string() } else { who(&entry) } detail=if hand(&entry) { "written to server.yaml directly".to_string() } else { entry.from.clone() }/>
            <Stat label="When" value=format::relative(entry.at) detail=""/>
        </Stats>
        <h3 class=label>"Diff of this change"</h3>
        <div class="mb-6 overflow-hidden rounded-card border border-border bg-surface">
            {move || match diff.get() {
                None => view! { <div class="p-4"><Skeleton lines=3/></div> }.into_any(),
                Some(Ok(lines)) => view! { <DiffView lines=lines testid="activity-diff"/> }.into_any(),
                Some(Err(e)) => view! { <p class="px-4 py-3 text-text-muted">{e}</p> }.into_any(),
            }}
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
            .map(|h| {
                rows_of(
                    &h,
                    s.can_write(),
                    &s.state
                        .with(|st| st.as_ref().map(|st| st.rev.clone()).unwrap_or_default()),
                )
            })
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
    let on_action = Callback::new(move |(id, action): (String, String)| {
        let entry = s.history.with_untracked(|h| {
            h.as_ref()
                .and_then(|h| h.iter().find(|e| e.id.to_string() == id).cloned())
        });
        match (action.as_str(), entry) {
            ("revert", Some(e)) => {
                s.edit.open_revert(s, revert_target(&e));
                route::go(Route::Review);
            }
            _ => route::go(Route::Activity(Some(id))),
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
            empty=(String::new(), String::new())
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
            let at = selected.as_ref().and_then(|r| h.iter().position(|e| &e.id.to_string() == r));
            match at {
                Some(i) => view! { <Version entry=h[i].clone() newer=h[..i].to_vec()/> }.into_any(),
                None => view! { <div class="max-lg:hidden"><EmptyState icon="history" title="Pick a version on the left" message="Each one shows what changed and can be reverted."/></div> }.into_any(),
            }
        }
    }
    };
    let detail = view! { <DetailPane back=Route::Activity(None)>{body}</DetailPane> }.into_any();
    master_detail(list, detail)
}

pub fn entry_of_rev(rev: &str) -> Option<String> {
    store().history.with_untracked(|h| {
        h.as_ref()?
            .iter()
            .find(|e| e.rev == rev)
            .map(|e| e.id.to_string())
    })
}

fn revert_target(entry: &AuditEntry) -> String {
    if entry.base.is_empty() {
        entry.rev.clone()
    } else {
        entry.base.clone()
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

    #[test]
    fn a_revert_is_titled_as_one_and_a_repeated_file_says_so() {
        let added = AuditEntry {
            rev: "b2s:aaaa1111".into(),
            base: "b2s:base".into(),
            visitor: "ops".into(),
            at: 100,
            changes: vec![burrow_admin_proto::plan::Change {
                kind: burrow_admin_proto::plan::ChangeKind::Add,
                path: vec!["visitors".into(), "c".into()],
                summary: "New visitor c (1 secret)".into(),
                part_of: None,
            }],
            ..AuditEntry::default()
        };
        let removed = AuditEntry {
            rev: "b2s:bbbb2222".into(),
            base: added.rev.clone(),
            visitor: "ops".into(),
            at: 200,
            ..AuditEntry::default()
        };
        let reverted = AuditEntry {
            rev: added.rev.clone(),
            base: removed.rev.clone(),
            reverts: Some(added.rev.clone()),
            at: 300,
            ..added.clone()
        };
        let by_hand = AuditEntry {
            reverts: None,
            visitor: "file".into(),
            ..reverted.clone()
        };
        assert_eq!(
            title_of(&reverted, &|_| burrow_admin_logic::wording::version_name(
                Some("10:00:00")
            )),
            "Reverted to the version of 10:00:00"
        );
        let list = [reverted, removed.clone(), added.clone()];
        assert!(repeats(&list, 0).is_none(), "the revert's title says it");
        let list = [by_hand, removed, added];
        assert_eq!(repeats(&list, 0).map(|o| o.at), Some(100));
        assert!(repeats(&list, 2).is_none());
    }

    #[test]
    fn a_kept_version_reverts_to_itself_and_a_change_to_its_base() {
        let kept = AuditEntry {
            rev: "b2s:kept".into(),
            ..AuditEntry::default()
        };
        assert_eq!(revert_target(&kept), "b2s:kept");
        let change = AuditEntry {
            rev: "b2s:after".into(),
            base: "b2s:before".into(),
            ..AuditEntry::default()
        };
        assert_eq!(revert_target(&change), "b2s:before");
    }
}
