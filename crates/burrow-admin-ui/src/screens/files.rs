use burrow_admin_logic::files::{files_of, folder_name, item_count, main_name};
use burrow_admin_proto::msg::ConfigFile;
use burrow_ui_kit::{Icon, Tone, tag_class};
use leptos::prelude::*;

use crate::route::{self, Route};
use crate::store::store;

fn sections_of(f: &ConfigFile) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for item in &f.items {
        let Some(section) = item.first() else {
            continue;
        };
        match out.iter_mut().find(|(s, _)| s == section) {
            Some((_, n)) => *n += 1,
            None => out.push((section.clone(), 1)),
        }
    }
    out
}

fn file_row(f: ConfigFile, main: &str) -> impl IntoView + use<> {
    let is_main = f.name == main;
    let state = match (&f.read_only, is_main) {
        (Some(why), _) => view! {
            <span class=tag_class(Tone::Neutral) title=why.clone() data-testid="file-read-only"><Icon name="lock" small=true/>"read-only"</span>
        }
        .into_any(),
        (None, true) => view! { <span class=tag_class(Tone::Accent)>"main file"</span> }.into_any(),
        (None, false) => view! { <span class=tag_class(Tone::Success)>"editable"</span> }.into_any(),
    };
    let why = f.read_only.clone().map(|w| {
        format!("Read-only on disk: {w}. Change it with the tool or editor that manages it.")
    });
    let links = sections_of(&f)
        .into_iter()
        .map(|(section, n)| {
            let (name, sec) = (f.name.clone(), section.clone());
            let label = format!("{n} {}", if n == 1 { crate::screens::create::singular(&section).to_string() } else { section.clone() });
            view! {
                <button type="button" class="cursor-pointer text-[13px] font-semibold text-accent-ink hover:underline" data-testid="file-items" on:click=move |_| route::go_search(Route::List(sec.clone()), &name)>{label}</button>
            }
        })
        .collect_view();
    view! {
        <li class="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-border px-4 py-3 first:border-t-0" data-testid="config-file" data-file=f.name.clone()>
            <span class="font-mono text-[13px] font-semibold">{f.name.clone()}</span>
            {state}
            <span class="text-[13px] text-text-muted">{item_count(&f)}</span>
            <span class="flex flex-wrap gap-3 sm:ml-auto">{links}</span>
            {why.map(|w| view! { <p class="w-full text-xs text-text-muted">{w}</p> })}
        </li>
    }
}

pub fn files_panel() -> impl IntoView {
    let s = store();
    move || {
        let (list, main, folder) = s
            .with_files(|i| (files_of(i), main_name(i), folder_name(i)))
            .unwrap_or_default();
        let rows = list.into_iter().map(|f| file_row(f, &main)).collect_view();
        view! {
            <div data-tab-panel="files" data-testid="files-panel">
                <section class="mb-4 overflow-hidden rounded-card border border-border bg-surface">
                    <ul>{rows}</ul>
                </section>
                <p class="text-[13px] text-text-muted">
                    {format!("Services, clients and visitors can also live in *.yaml files in {folder}/ next to {main}. A file with no write permission is read-only here; Split and Merge in an item's ⋯ menu move it between files.")}
                </p>
            </div>
        }
    }
}
