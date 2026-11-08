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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Needs {
    #[default]
    Wasm,
    WasmSimd,
}

const SIMD_PROBE: [u8; 31] = [
    0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 10, 1, 8, 0, 65, 0, 253,
    15, 253, 98, 11,
];

pub fn can_run(needs: Needs) -> bool {
    match needs {
        Needs::Wasm => true,
        Needs::WasmSimd => {
            let probe = js_sys::Uint8Array::from(SIMD_PROBE.as_slice());
            js_sys::WebAssembly::validate(&probe).unwrap_or(false)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Loading,
    Failed,
    Unsupported,
    Ready,
}

fn after_failure(runs: bool) -> Phase {
    if runs {
        Phase::Failed
    } else {
        Phase::Unsupported
    }
}

#[component]
pub fn Deferred<F, Fut>(
    load: F,
    #[prop(into)] what: String,
    #[prop(optional)] needs: Needs,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView
where
    F: Fn() -> Fut + 'static,
    Fut: Future<Output = Result<AnyView, LoadFailed>> + 'static,
{
    let runs = can_run(needs);
    let phase = RwSignal::new(if runs {
        Phase::Loading
    } else {
        Phase::Unsupported
    });
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
                        Err(LoadFailed) => phase.set(after_failure(runs)),
                    }
                })
            });
        }));
    if runs {
        start.with_value(|f| f());
    }
    let loading = format!("Loading {what}…");
    let failed = format!("Could not load {what}");
    let unsupported = format!("This browser cannot load {what}");
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
                Phase::Unsupported => view! {
                    <Callout tone=Tone::Warning title=unsupported.clone()>
                        <p>"It needs WebAssembly SIMD: use a current Chrome, Edge or Firefox, or Safari 16.4 or later."</p>
                    </Callout>
                }.into_any(),
                Phase::Ready => slot.try_update_value(Option::take).flatten().unwrap_or_else(|| ().into_any()),
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_load_offers_a_retry_only_where_the_browser_can_run_the_chunk() {
        assert_eq!(after_failure(true), Phase::Failed);
        assert_eq!(after_failure(false), Phase::Unsupported);
        assert_eq!(Needs::default(), Needs::Wasm);
        assert!(can_run(Needs::Wasm));
        assert_eq!(&SIMD_PROBE[..8], b"\0asm\x01\0\0\0");
    }
}
