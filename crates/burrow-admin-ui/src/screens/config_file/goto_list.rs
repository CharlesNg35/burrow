use burrow_admin_logic::goto::{Found, Item, Kind, Sources, find};
use burrow_admin_proto::text_search::{self, MAX_LIMIT, TextMatch};
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{FileName, Icon, Kbd, PickHead, PickOption, marked};
use config_editor_leptos::{Language, TextPosition};
use leptos::prelude::*;

use super::goto::{GoTo, PLACEHOLDER, problem_files};
use crate::edit::{Spot, TextEdit};
use crate::store::{Store, store};

const FIRST: usize = 8;
const MORE: usize = 50;
const LIST: &str = "file-goto-list";

#[derive(Clone, PartialEq)]
enum Row {
    Head(Kind),
    Hit(Kind, Found),
    More(Kind, usize),
    Note(String),
}

fn items(s: Store, main: &str) -> Vec<Item> {
    let mut out: Vec<Item> = s
        .with_files(|i| {
            i.files
                .iter()
                .flat_map(|f| {
                    f.items.iter().map(|p| Item {
                        path: p.clone(),
                        file: f.name.clone(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let doc = s.edit.doc();
    for section in [
        ServiceEntry::section(),
        ClientEntry::section(),
        VisitorEntry::section(),
    ] {
        for name in doc
            .get(section)
            .and_then(|v| v.as_object())
            .into_iter()
            .flat_map(|m| m.keys())
        {
            let path = vec![section.to_string(), name.clone()];
            if !out.iter().any(|i| i.path == path) {
                out.push(Item {
                    path,
                    file: main.to_string(),
                });
            }
        }
    }
    out
}

fn rows(g: GoTo, t: TextEdit) -> Vec<Row> {
    let s = store();
    let mut files: Vec<String> = t
        .text()
        .map(|x| x.files.iter().map(|f| f.name.clone()).collect())
        .unwrap_or_default();
    files.extend(t.new_files.get());
    let main = files.first().cloned().unwrap_or_default();
    let items = items(s, &main);
    let recent = t.recent.get();
    let problems = problem_files(t);
    let (text, cut) = in_text(g, t, &files);
    let src = Sources {
        files: &files,
        items: &items,
        recent: &recent,
        problems: &problems,
        text: &text,
    };
    let shown = g.shown.get();
    let mut out = Vec::new();
    for (kind, hits) in find(&g.q.get(), &src) {
        let limit = shown.get(&kind).copied().unwrap_or(FIRST);
        out.push(Row::Head(kind));
        let total = hits.len();
        out.extend(hits.into_iter().take(limit).map(|h| Row::Hit(kind, h)));
        if total > limit {
            out.push(Row::More(kind, total - limit));
        }
        if kind == Kind::Text && cut {
            out.push(Row::Note(format!(
                "The first {MAX_LIMIT} matches; type more to narrow them"
            )));
        }
    }
    out
}

fn in_text(g: GoTo, t: TextEdit, files: &[String]) -> (Vec<TextMatch>, bool) {
    let q = g.text_q.get();
    if q.trim().is_empty() || q.trim() != g.q.with(|x| x.trim().to_string()) {
        return (Vec::new(), false);
    }
    t.values.with(|v| {
        let texts: Vec<(&str, &str)> = files
            .iter()
            .filter_map(|f| Some((f.as_str(), v.get(f)?.as_str())))
            .collect();
        text_search::find(&texts, &q, MAX_LIMIT as usize)
    })
}

fn holds_a_field(line: &str) -> bool {
    let l = line.trim_start();
    !l.is_empty() && !l.starts_with('#') && !l.starts_with("//")
}

pub fn field_at(language: Language, text: &str, line: u32, col: u32) -> Option<String> {
    let at = |line: usize, col: usize| {
        language
            .path_at(text, TextPosition::new(line, col))
            .filter(|p| !p.is_root())
            .map(|p| p.to_pointer())
    };
    let lines: Vec<&str> = text.split('\n').collect();
    let here = lines
        .get(line.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("");
    if holds_a_field(here)
        && let Some(p) = at(line as usize, col as usize)
    {
        return Some(p);
    }
    lines
        .iter()
        .enumerate()
        .skip(line as usize)
        .filter(|(_, l)| holds_a_field(l))
        .find_map(|(i, l)| {
            at(
                i + 1,
                l.chars().take_while(|c| c.is_whitespace()).count() + 1,
            )
        })
}

fn spot_of(t: TextEdit, file: &str, (line, col, end): (u32, u32, u32)) -> Spot {
    let own = super::edit_as::own_label(file);
    let shown = t
        .shown
        .with_untracked(|s| s.get(file).cloned())
        .unwrap_or_else(|| own.to_string());
    if shown == own {
        return Spot::Range(line, col, end);
    }
    let language = Language::from_path(file).unwrap_or(Language::YAML);
    t.values
        .with_untracked(|v| {
            v.get(file)
                .and_then(|text| field_at(language, text, line, col))
        })
        .map_or(Spot::Here, Spot::Pointer)
}

fn pick(g: GoTo, t: TextEdit, row: Row) {
    match row {
        Row::Head(_) | Row::Note(_) => {}
        Row::More(kind, _) => g.shown.update(|m| {
            let n = m.entry(kind).or_insert(FIRST);
            *n += MORE;
        }),
        Row::Hit(_, f) => {
            g.open.set(false);
            match (f.path, f.line) {
                (Some(p), _) => t.open_at(&f.file, Spot::Path(p)),
                (None, Some((l, col, end))) => {
                    t.open_at(&f.file, spot_of(t, &f.file, (l, col, end)))
                }
                (None, None) => t.open_at(&f.file, Spot::Here),
            }
        }
    }
}

fn keys(g: GoTo, t: TextEdit, list: Memo<Vec<Row>>) -> impl Fn(web_sys::KeyboardEvent) + Copy {
    move |e: web_sys::KeyboardEvent| {
        let picks: Vec<usize> = list.with(|l| {
            l.iter()
                .enumerate()
                .filter(|(_, r)| !matches!(r, Row::Head(_) | Row::Note(_)))
                .map(|(i, _)| i)
                .collect()
        });
        let at = picks.iter().position(|i| *i == g.active.get_untracked());
        match e.key().as_str() {
            "ArrowDown" => {
                e.prevent_default();
                let next = at.map_or(0, |a| (a + 1).min(picks.len().saturating_sub(1)));
                if let Some(i) = picks.get(next) {
                    g.active.set(*i);
                }
            }
            "ArrowUp" => {
                e.prevent_default();
                if let Some(i) = picks.get(at.map_or(0, |a| a.saturating_sub(1))) {
                    g.active.set(*i);
                }
            }
            "Enter" => {
                e.prevent_default();
                let row = list.with(|l| l.get(g.active.get_untracked()).cloned());
                if let Some(row) = row {
                    pick(g, t, row);
                }
            }
            _ => {}
        }
    }
}

fn first_pick(list: &[Row]) -> usize {
    list.iter()
        .position(|r| !matches!(r, Row::Head(_) | Row::Note(_)))
        .unwrap_or(0)
}

fn option(g: GoTo, t: TextEdit, i: usize, row: Row, needle: &str) -> AnyView {
    let active = Signal::derive(move || g.active.get() == i);
    let hover = Callback::new(move |_| g.active.set(i));
    let id = format!("file-goto-opt-{i}");
    match row.clone() {
        Row::Head(kind) => view! { <PickHead text=kind.title()/> }.into_any(),
        Row::Note(text) => view! { <li class="px-2.5 py-1.5 text-xs text-text-muted" role="presentation" data-testid="file-goto-note">{text}</li> }.into_any(),
        Row::More(_, n) => view! {
            <PickOption icon="more" id=id active=active on_hover=hover on_pick=Callback::new(move |_| pick(g, t, row.clone())) testid="file-goto-more">
                {format!("… {n} more")}
            </PickOption>
        }
        .into_any(),
        Row::Hit(kind, f) => {
            let label = if kind == Kind::File {
                view! { <FileName name=f.label.clone() mark=needle.to_string()/> }.into_any()
            } else {
                marked(&f.label, needle)
            };
            view! {
                <PickOption icon=kind.icon() id=id active=active on_hover=hover on_pick=Callback::new(move |_| pick(g, t, row.clone())) note=f.note.clone() testid="file-goto-hit">
                    {label}
                </PickOption>
            }
            .into_any()
        }
    }
}

#[leptos::wasm_split::wasm_split(config_editor, wasm_split_path = ::leptos::wasm_split, fallible)]
pub fn goto_list(g: GoTo, t: TextEdit) -> Result<AnyView, burrow_ui_kit::LoadFailed> {
    let list = Memo::new(move |_| if g.open.get() { rows(g, t) } else { Vec::new() });
    Effect::new(move |_| {
        g.q.track();
        g.active.set(list.with_untracked(|l| first_pick(l)));
    });
    request_animation_frame(|| {
        if let Some(el) = document().get_element_by_id("file-goto-input")
            && let Ok(el) = wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(el)
        {
            let _ = el.focus();
        }
    });
    let on_key = keys(g, t, list);
    let body = move || {
        let needle = g.q.get().trim().to_lowercase();
        let rows = list.get();
        if rows.is_empty() {
            return view! { <li class="px-2.5 py-3 text-sm text-text-muted" role="presentation">"Nothing matches"</li> }.into_any();
        }
        rows.into_iter()
            .enumerate()
            .map(|(i, r)| option(g, t, i, r, &needle))
            .collect_view()
            .into_any()
    };
    Ok(view! {
        <div on:keydown=on_key>
            <div class="flex items-center gap-2.5 border-b border-border px-3">
                <Icon name="search"/>
                <input
                    id="file-goto-input"
                    type="text"
                    role="combobox"
                    aria-expanded="true"
                    aria-controls=LIST
                    aria-autocomplete="list"
                    aria-activedescendant=move || format!("file-goto-opt-{}", g.active.get())
                    aria-label="Go to a file, service, client or visitor, or find text in the files"
                    placeholder=PLACEHOLDER
                    autocomplete="off"
                    class="h-11 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-text-faint"
                    prop:value=move || g.q.get()
                    on:input:target=move |e| g.typed(e.target().value())
                />
                <span class="max-sm:hidden"><Kbd text="esc"/></span>
            </div>
            <ul id=LIST class="max-h-[min(60vh,420px)] overflow-y-auto p-1.5" role="listbox" aria-label="Go to">{body}</ul>
        </div>
    }
    .into_any())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# top\nservices:\n  # the web site\n  web:\n    target: 80\n\n";

    #[test]
    fn a_text_match_opens_at_the_field_that_holds_it() {
        assert_eq!(
            field_at(Language::YAML, TEXT, 5, 13).as_deref(),
            Some("/services/web/target")
        );
    }

    #[test]
    fn a_match_on_a_comment_opens_at_the_next_field() {
        assert_eq!(
            field_at(Language::YAML, TEXT, 3, 5).as_deref(),
            Some("/services/web")
        );
        assert_eq!(
            field_at(Language::YAML, TEXT, 1, 3).as_deref(),
            Some("/services")
        );
        assert_eq!(
            field_at(Language::YAML, TEXT, 6, 1),
            None,
            "nothing after it"
        );
    }
}
