use std::future::Future;

use leptos::prelude::*;
use leptos::wasm_split::SplitLoaderError;

use crate::basic::Tone;
use crate::button::{Button, ButtonSize};
use crate::feedback::{Callout, Spinner};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadFailed;

impl From<SplitLoaderError> for LoadFailed {
    fn from(_: SplitLoaderError) -> Self {
        LoadFailed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Loading,
    Failed,
    Ready,
}

#[component]
pub fn Deferred<F, Fut>(
    load: F,
    #[prop(into)] what: String,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView
where
    F: Fn() -> Fut + 'static,
    Fut: Future<Output = Result<AnyView, LoadFailed>> + 'static,
{
    let phase = RwSignal::new(Phase::Loading);
    let slot = StoredValue::new_local(None::<AnyView>);
    let owner = Owner::current().unwrap_or_default();
    let start: StoredValue<Box<dyn Fn()>, LocalStorage> =
        StoredValue::new_local(Box::new(move || {
            phase.set(Phase::Loading);
            let pending = load();
            owner.with(|| {
                leptos::task::spawn_local_scoped_with_cancellation(async move {
                    match pending.await {
                        Ok(view) => {
                            slot.set_value(Some(view));
                            phase.set(Phase::Ready);
                        }
                        Err(LoadFailed) => phase.set(Phase::Failed),
                    }
                })
            });
        }));
    start.with_value(|f| f());
    let loading = format!("Loading {what}…");
    let failed = format!("Could not load {what}");
    view! {
        <div class="contents" data-testid=(!testid.is_empty()).then_some(testid) data-phase=move || format!("{:?}", phase.get()).to_lowercase()>
            {move || match phase.get() {
                Phase::Loading => view! { <div class="p-4"><Spinner label=loading.clone()/></div> }.into_any(),
                Phase::Failed => {
                    view! {
                        <Callout tone=Tone::Danger title=failed.clone()>
                            <p class="mb-2">"The connection to burrow admin may be down. Check it and try again."</p>
                            <Button label="Try again" size=ButtonSize::Small on_click=move |()| start.with_value(|f| f())/>
                        </Callout>
                    }.into_any()
                }
                Phase::Ready => slot.try_update_value(Option::take).flatten().unwrap_or_else(|| ().into_any()),
            }}
        </div>
    }
}
