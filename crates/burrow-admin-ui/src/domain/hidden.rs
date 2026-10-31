use burrow_admin_logic::tree::inner;
use burrow_admin_proto::schema::SemType;
use burrow_admin_proto::value::{as_secret, display};
use burrow_ui_kit::Button;
use leptos::prelude::*;

use crate::form::VALUE_BOX;
use crate::store::store;

use super::FieldCtx;

pub(super) fn hidden_or(fc: FieldCtx, edit: fn(FieldCtx) -> AnyView) -> AnyView {
    let e = store().edit;
    let kept = e
        .peek(&fc.path)
        .filter(|v| as_secret(v).is_some())
        .filter(|_| !matches!(inner(&fc.node).sem, SemType::SecretRef(_)));
    let Some(kept) = kept else {
        return edit(fc);
    };
    let replacing = RwSignal::new(false);
    let shown = display(&kept);
    let path = fc.path.clone();
    let keep = move |()| {
        e.local_error(&path, None);
        e.set(&path, Some(kept.clone()));
        replacing.set(false);
    };
    view! {
        <div data-hidden-value="">
            {move || if replacing.get() {
                view! {
                    <div class="flex flex-wrap items-start gap-2">
                        <div class="min-w-0 flex-1 max-sm:basis-full">{edit(fc.clone())}</div>
                        <Button label="Keep the current value" small=true testid="keep-hidden" on_click=keep.clone()/>
                    </div>
                }
                .into_any()
            } else {
                view! {
                    <div class="flex flex-wrap items-center gap-2">
                        <div class=format!("{VALUE_BOX} min-w-0 flex-1 gap-2 overflow-hidden") id=fc.id.clone()>
                            <span class="shrink-0">{shown.clone()}</span>
                            <span class="min-w-0 truncate font-sans text-xs text-text-muted max-sm:sr-only">"hidden; kept unless you replace it"</span>
                        </div>
                        <Button label="Replace" small=true testid="replace-hidden" on_click=move |()| replacing.set(true)/>
                    </div>
                }
                .into_any()
            }}
        </div>
    }
    .into_any()
}
