use burrow_admin_logic::files::{change_label, empty_offer};
use burrow_admin_proto::plan::{FileChange, FileDiff};
use burrow_ui_kit::{Button, DiffView};
use leptos::prelude::*;

use crate::screens::activity::diff_lines;
use crate::store::store;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Offer {
    Not,
    Draft,
    Text,
}

impl Offer {
    fn on(self) -> bool {
        self != Offer::Not
    }

    fn dropped(self, file: &str) -> bool {
        let s = store();
        match self {
            Offer::Not => false,
            Offer::Draft => s.edit.files.with_untracked(|m| m.drops.contains(file)),
            Offer::Text => s.text.drops.with_untracked(|d| d.contains(file)),
        }
    }

    fn set(self, file: &str, drop: bool) {
        let s = store();
        match self {
            Offer::Not => {}
            Offer::Draft => s.edit.drop_file(file, drop),
            Offer::Text => {
                s.text.drop_file(file, drop);
                s.edit.open_text(s);
            }
        }
    }
}

pub(crate) fn file_diffs(files: &[FileDiff], split: RwSignal<bool>, offer: Offer) -> AnyView {
    files
        .iter()
        .map(|f| {
            let lines = diff_lines(&f.diff);
            let same = f.change == FileChange::Same;
            let head = format!("{}: {}", f.file, change_label(&f.change));
            let empty = (offer.on() && f.left_empty == Some(true)).then(|| {
                let file = f.file.clone();
                view! {
                    <div class="flex flex-wrap items-center gap-2 border-t border-border px-4 py-2.5 text-[13px] text-text-muted" data-testid="empty-file">
                        <span class="min-w-0 [overflow-wrap:anywhere]">{empty_offer(&file)}</span>
                        <span class="ml-auto"></span>
                        <Button label="Delete the file" variant=burrow_ui_kit::ButtonLook::Secondary testid="drop-file" on_click=move |_| offer.set(&file, true)/>
                    </div>
                }
            });
            let kept = (f.change == FileChange::Remove && offer.dropped(&f.file))
            .then(|| {
                let file = f.file.clone();
                view! {
                    <div class="flex items-center gap-2 border-t border-border px-4 py-2.5 text-[13px]">
                        <span class="ml-auto"></span>
                        <Button label="Keep the empty file" variant=burrow_ui_kit::ButtonLook::Ghost testid="keep-file" on_click=move |_| offer.set(&file, false)/>
                    </div>
                }
            });
            view! {
                <section class="mb-3 overflow-hidden rounded-card border border-border bg-surface" data-testid="file-diff" data-file=f.file.clone()>
                    <h4 class="border-b border-border px-4 py-2 font-mono text-[12.5px] text-text-muted [overflow-wrap:anywhere]" class:border-b-0=same>{head}</h4>
                    {(!same && !lines.is_empty()).then(|| view! { <DiffView lines=lines split=split testid="review-diff"/> })}
                    {empty}
                    {kept}
                </section>
            }
        })
        .collect_view()
        .into_any()
}
