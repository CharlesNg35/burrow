use burrow_admin_logic::files::{change_label, empty_offer};
use burrow_admin_proto::plan::{FileChange, FileDiff};
use burrow_ui_kit::{Button, DiffView};
use leptos::prelude::*;

use crate::screens::activity::diff_lines;
use crate::store::store;

pub(crate) fn file_diffs(files: &[FileDiff], split: RwSignal<bool>, offer: bool) -> AnyView {
    let s = store();
    files
        .iter()
        .map(|f| {
            let lines = diff_lines(&f.diff);
            let same = f.change == FileChange::Same;
            let head = format!("{}: {}", f.file, change_label(&f.change));
            let empty = (offer && f.left_empty == Some(true)).then(|| {
                let file = f.file.clone();
                view! {
                    <div class="flex flex-wrap items-center gap-2 border-t border-border px-4 py-2.5 text-[13px] text-text-muted" data-testid="empty-file">
                        <span>{empty_offer(&file)}</span>
                        <span class="ml-auto"></span>
                        <Button label=format!("Delete {file}") variant="secondary" testid="drop-file" on_click=move |_| s.edit.drop_file(&file, true)/>
                    </div>
                }
            });
            let kept = (offer && f.change == FileChange::Remove
                && s.edit.files.with_untracked(|m| m.drops.contains(&f.file)))
            .then(|| {
                let file = f.file.clone();
                view! {
                    <div class="flex items-center gap-2 border-t border-border px-4 py-2.5 text-[13px]">
                        <span class="ml-auto"></span>
                        <Button label="Keep the empty file" variant="ghost" testid="keep-file" on_click=move |_| s.edit.drop_file(&file, false)/>
                    </div>
                }
            });
            view! {
                <section class="mb-3 overflow-hidden rounded-card border border-border bg-surface" data-testid="file-diff" data-file=f.file.clone()>
                    <h4 class="border-b border-border px-4 py-2 font-mono text-[12.5px] text-text-muted" class:border-b-0=same>{head}</h4>
                    {(!same && !lines.is_empty()).then(|| view! { <DiffView lines=lines split=split testid="review-diff"/> })}
                    {empty}
                    {kept}
                </section>
            }
        })
        .collect_view()
        .into_any()
}
