use std::collections::{BTreeMap, BTreeSet, HashMap};

use burrow_ui_kit::{FileName, Icon, LABEL, Select, Tone, tag_class};
use leptos::prelude::*;

use crate::store::{Grouping, ListGroups, store};

use super::Row;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub name: String,
    pub read_only: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub file: String,
    pub read_only: Option<String>,
    pub rows: Vec<usize>,
    pub problems: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grouped {
    pub loose: Vec<usize>,
    pub sections: Vec<Section>,
}

pub fn group(files: &[FileRef], rows: &[Row]) -> Grouped {
    let main = files.first().map_or("", |f| f.name.as_str());
    let at: HashMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.as_str(), i))
        .collect();
    let mut known: Vec<Vec<usize>> = vec![Vec::new(); files.len()];
    let mut other: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut loose = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if r.is_placeholder() {
            loose.push(i);
            continue;
        }
        let file = if r.file.is_empty() { main } else { &r.file };
        match at.get(file) {
            Some(&k) => known[k].push(i),
            None => other.entry(file).or_default().push(i),
        }
    }
    let problems = |ix: &[usize]| ix.iter().map(|&i| rows[i].problems).sum();
    let mut sections: Vec<Section> = files
        .iter()
        .zip(known)
        .filter(|(_, ix)| !ix.is_empty())
        .map(|(f, ix)| Section {
            file: f.name.clone(),
            read_only: f.read_only.clone(),
            problems: problems(&ix),
            rows: ix,
        })
        .collect();
    sections.extend(other.into_iter().map(|(f, ix)| Section {
        file: f.to_string(),
        read_only: None,
        problems: problems(&ix),
        rows: ix,
    }));
    Grouped { loose, sections }
}

pub fn is_open(sec: &Section, searching: bool, closed: &BTreeSet<String>, holds: bool) -> bool {
    searching || holds || !closed.contains(&sec.file)
}

pub fn order(g: &Grouped, open: impl Fn(&Section) -> bool) -> Vec<usize> {
    g.loose
        .iter()
        .copied()
        .chain(
            g.sections
                .iter()
                .filter(|s| open(s))
                .flat_map(|s| s.rows.iter().copied()),
        )
        .collect()
}

pub fn files() -> Vec<FileRef> {
    let s = store();
    if !s.files_on() {
        return Vec::new();
    }
    s.with_files(|i| {
        i.files
            .iter()
            .map(|f| FileRef {
                name: f.name.clone(),
                read_only: f.read_only.clone(),
            })
            .collect()
    })
    .unwrap_or_default()
}

pub fn mode(kind: &'static str) -> Grouping {
    let n = files().len();
    if n < 2 {
        return Grouping::Flat;
    }
    store()
        .list_groups
        .with(|m| m.get(kind).and_then(|g| g.mode))
        .unwrap_or(Grouping::default_for(n))
}

pub fn closed(kind: &'static str) -> BTreeSet<String> {
    store()
        .list_groups
        .with(|m| m.get(kind).map(|g| g.closed.clone()))
        .unwrap_or_default()
}

fn update(kind: &'static str, f: impl FnOnce(&mut ListGroups)) {
    store()
        .list_groups
        .update(|m| f(m.entry(kind).or_default()));
}

pub fn control(kind: &'static str) -> impl IntoView {
    let shown = Memo::new(move |_| files().len() > 1);
    let value = Signal::derive(move || {
        match mode(kind) {
            Grouping::Flat => "none",
            Grouping::ByFile => "file",
        }
        .to_string()
    });
    let options = Signal::stored(vec![
        ("none".to_string(), "None".to_string()),
        ("file".to_string(), "By file".to_string()),
    ]);
    let pick = Callback::new(move |v: String| {
        let m = if v == "file" {
            Grouping::ByFile
        } else {
            Grouping::Flat
        };
        update(kind, |g| g.mode = Some(m));
    });
    move || {
        shown.get().then(|| {
            view! {
                <label class="flex shrink-0 items-center gap-1.5 text-xs text-text-muted" data-testid="list-group">
                    <span>"Group"</span>
                    <span class="w-[6.5rem]">
                        <Select id=format!("list-group-{kind}") options=options value=value on_change=pick label="Group the list" small=true/>
                    </span>
                </label>
            }
        })
    }
}

struct Head {
    file: String,
    read_only: Option<String>,
    count: usize,
    problems: usize,
}

fn header(kind: &'static str, h: Head, open: bool, searching: bool, panel: String) -> AnyView {
    let Head {
        file,
        read_only,
        count,
        problems,
    } = h;
    let label = file.clone();
    let toggle = file.clone();
    let lock = read_only.map(|why| {
        let tip = format!("Read-only on disk: {why}");
        view! { <span class="text-text-muted" title=tip.clone() aria-label=tip data-testid="group-lock"><Icon name="lock" small=true/></span> }
    });
    let problems = (!open && problems > 0).then(|| {
        let text = if problems == 1 {
            "1 problem".to_string()
        } else {
            format!("{problems} problems")
        };
        view! { <span class=format!("{} normal-case tracking-normal", tag_class(Tone::Warning)) data-testid="group-problems">{text}</span> }
    });
    view! {
        <button
            type="button"
            class=format!("relative flex w-full cursor-pointer items-center gap-1.5 px-4 pt-3.5 pb-1.5 text-left outline-none hover:text-text disabled:cursor-default focus-visible:after:absolute focus-visible:after:inset-x-2 focus-visible:after:top-2 focus-visible:after:-bottom-0.5 focus-visible:after:rounded-[11px] focus-visible:after:border focus-visible:after:border-accent-line focus-visible:after:ring-3 focus-visible:after:ring-accent-weak {LABEL}")
            aria-expanded=open.to_string()
            aria-controls=panel
            disabled=searching
            title=searching.then_some("Clear the search to fold this file")
            data-file=file
            data-testid="file-group"
            on:click=move |_| {
                let f = toggle.clone();
                update(kind, move |g| {
                    if !g.closed.remove(&f) {
                        g.closed.insert(f);
                    }
                });
            }
        >
            <Icon name=if open { "chevron-down" } else { "chevron-right" } small=true/>
            <span class="flex min-w-0 font-mono text-[13px] normal-case" data-testid="group-file"><FileName name=label/></span>
            {lock}
            {problems}
            <span class="ml-auto shrink-0" data-testid="group-count">{count}</span>
        </button>
    }
    .into_any()
}

fn in_its_file(mut r: Row) -> Row {
    r.kept = r.kept.as_ref().and_then(super::Kept::in_its_file);
    r
}

pub fn render(
    kind: &'static str,
    all: Memo<Vec<Row>>,
    visible: Memo<Vec<Row>>,
    searching: Signal<bool>,
    cards: super::list_rows::Cards,
) -> AnyView {
    let layout = Memo::new(move |_| all.with(|a| group(&files(), a)));
    let seen = Memo::new(move |_| visible.with(|v| group(&files(), v)));
    let loose = move || {
        layout.with(|g| all.with(|a| g.loose.iter().map(|&i| a[i].clone()).collect::<Vec<_>>()))
    };
    let names = move || {
        layout.with(|g| {
            g.sections
                .iter()
                .map(|s| s.file.clone())
                .collect::<Vec<_>>()
        })
    };
    let section = move |file: String| {
        let at = file.clone();
        let mine =
            Memo::new(move |_| seen.with(|g| g.sections.iter().find(|s| s.file == at).cloned()));
        let rows_of = file.clone();
        let rows = move || {
            layout.with(|g| {
                all.with(|a| {
                    g.sections
                        .iter()
                        .find(|s| s.file == rows_of)
                        .map(|s| {
                            s.rows
                                .iter()
                                .map(|&i| in_its_file(a[i].clone()))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
            })
        };
        let read_only = layout.with_untracked(|g| {
            g.sections
                .iter()
                .find(|s| s.file == file)
                .and_then(|s| s.read_only.clone())
        });
        let folded = file.clone();
        let open = Memo::new(move |_| {
            let closed = closed(kind);
            let Some(sec) = mine.get() else {
                return !closed.contains(&folded);
            };
            let holds = cards.selected.with(|k| {
                k.as_deref()
                    .is_some_and(|k| visible.with(|v| sec.rows.iter().any(|&i| v[i].key == k)))
            });
            is_open(&sec, searching.get(), &closed, holds)
        });
        let n = layout
            .with_untracked(|g| g.sections.iter().position(|s| s.file == file))
            .unwrap_or(0);
        let panel = format!("group-{kind}-{n}");
        let id = panel.clone();
        let name = file.clone();
        view! {
            <div data-group-file=file.clone() hidden=move || mine.with(Option::is_none)>
                {move || {
                    let (count, problems) = mine.with(|m| m.as_ref().map_or((0, 0), |s| (s.rows.len(), s.problems)));
                    let h = Head { file: name.clone(), read_only: read_only.clone(), count, problems };
                    header(kind, h, open.get(), searching.get(), id.clone())
                }}
                <div id=panel hidden=move || !open.get()>
                    {move || open.get().then(|| { let rows = rows.clone(); view! { <For each=rows key=super::list_rows::print children=move |r| super::list_rows::card(r, cards)/> } })}
                </div>
            </div>
        }
    };
    view! {
        <For each=loose key=super::list_rows::print children=move |r| super::list_rows::card(r, cards)/>
        <For each=names key=|f| f.clone() children=section/>
    }
    .into_any()
}

pub fn navigable(
    kind: &'static str,
    list: &[Row],
    searching: bool,
    selected: Option<&str>,
) -> Vec<Row> {
    let g = group(&files(), list);
    let closed = closed(kind);
    order(&g, |s| {
        is_open(
            s,
            searching,
            &closed,
            selected.is_some_and(|k| s.rows.iter().any(|&i| list[i].key == k)),
        )
    })
    .into_iter()
    .map(|i| list[i].clone())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, file: &str, problems: usize) -> Row {
        Row {
            key: key.into(),
            file: file.into(),
            problems,
            ..Row::default()
        }
    }

    fn files(names: &[&str]) -> Vec<FileRef> {
        names
            .iter()
            .map(|n| FileRef {
                name: n.to_string(),
                read_only: n.contains("ro").then(|| "not writable by burrow".into()),
            })
            .collect()
    }

    #[test]
    fn sections_follow_the_read_order_and_skip_files_without_items() {
        let rows = vec![
            row("b", "server.d/b.yaml", 0),
            row("main1", "server.yaml", 1),
            row("a", "server.d/a-ro.yaml", 2),
            row("plain", "", 0),
            row("a2", "server.d/a-ro.yaml", 1),
            row("moved", "server.d/new.yaml", 0),
            row(crate::screens::NEW_ROW, "", 0),
        ];
        let g = group(
            &files(&[
                "server.yaml",
                "server.d/a-ro.yaml",
                "server.d/b.yaml",
                "server.d/empty.yaml",
            ]),
            &rows,
        );
        assert_eq!(g.loose, vec![6], "the new-item row stays above the files");
        let names: Vec<&str> = g.sections.iter().map(|s| s.file.as_str()).collect();
        assert_eq!(
            names,
            [
                "server.yaml",
                "server.d/a-ro.yaml",
                "server.d/b.yaml",
                "server.d/new.yaml"
            ]
        );
        assert_eq!(
            g.sections[0].rows,
            vec![1, 3],
            "an item without a file is in the main file"
        );
        assert_eq!(g.sections[1].rows, vec![2, 4]);
        assert_eq!(g.sections[1].problems, 3);
        assert_eq!(
            g.sections[1].read_only.as_deref(),
            Some("not writable by burrow")
        );
        assert_eq!(
            g.sections[3].read_only, None,
            "a file only in the draft is not read-only"
        );
    }

    #[test]
    fn a_search_opens_every_section_and_the_selection_keeps_its_own_open() {
        let rows = vec![row("x", "server.yaml", 0), row("y", "server.d/b.yaml", 0)];
        let g = group(&files(&["server.yaml", "server.d/b.yaml"]), &rows);
        let closed: BTreeSet<String> = ["server.d/b.yaml".to_string()].into();
        let b = &g.sections[1];
        assert!(!is_open(b, false, &closed, false));
        assert!(is_open(b, true, &closed, false), "a search opens it");
        assert!(is_open(b, false, &closed, true), "the selected item shows");
        assert!(is_open(&g.sections[0], false, &closed, false));
        assert_eq!(order(&g, |s| is_open(s, false, &closed, false)), vec![0]);
        assert_eq!(order(&g, |s| is_open(s, true, &closed, false)), vec![0, 1]);
    }

    #[test]
    fn three_files_or_more_group_by_file_by_default() {
        assert_eq!(Grouping::default_for(2), Grouping::Flat);
        assert_eq!(Grouping::default_for(3), Grouping::ByFile);
    }

    #[test]
    fn grouping_a_thousand_items_in_a_hundred_files_is_one_pass() {
        let names: Vec<String> = (0..100).map(|i| format!("server.d/f{i:03}.yaml")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let rows: Vec<Row> = (0..1000)
            .map(|i| row(&format!("s{i}"), &names[i % 100], (i / 100) % 2))
            .collect();
        let t = std::time::Instant::now();
        let mut g = group(&files(&refs), &rows);
        for _ in 0..99 {
            g = group(&files(&refs), &rows);
        }
        let per = t.elapsed() / 100;
        assert_eq!(g.sections.len(), 100);
        assert!(
            g.sections
                .iter()
                .all(|s| s.rows.len() == 10 && s.problems == 5)
        );
        assert!(
            per < std::time::Duration::from_millis(20),
            "{per:?} per grouping"
        );
        println!("grouping 1000 items in 100 files: {per:?}");
    }
}
