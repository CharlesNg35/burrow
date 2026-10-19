use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::basic::button_class;
use crate::icon::Icon;

pub fn copy_text(text: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.navigator().clipboard().write_text(text);
    }
}

pub fn download_text(name: &str, text: &str) {
    let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(text));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("text/plain");
    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    let doc = web_sys::window().and_then(|w| w.document());
    if let Some(a) = doc
        .and_then(|d| d.create_element("a").ok())
        .and_then(|e| e.dyn_into::<web_sys::HtmlAnchorElement>().ok())
    {
        a.set_href(&url);
        a.set_download(name);
        a.click();
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

#[component]
pub fn CodeBlock(
    #[prop(into)] text: Signal<String>,
    #[prop(optional, into)] file_name: Option<String>,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    let copied = RwSignal::new(false);
    let name = file_name.clone();
    view! {
        <div class="overflow-hidden rounded-card border border-border" data-testid=(!testid.is_empty()).then_some(testid)>
            <div class="flex items-center gap-2 border-b border-border bg-surface-2 px-3 py-1.5">
                <span class="flex-1 font-mono text-sm">{file_name.clone().unwrap_or_default()}</span>
                <button
                    type="button"
                    class=button_class("ghost", true)
                    on:click=move |_| {
                        copy_text(&text.get_untracked());
                        copied.set(true);
                    }
                >
                    <Icon name="copy"/>
                    {move || if copied.get() { "Copied" } else { "Copy" }}
                </button>
                {name
                    .map(|n| {
                        let n2 = n.clone();
                        view! {
                            <button
                                type="button"
                                class=button_class("ghost", true)
                                on:click=move |_| download_text(&n2, &text.get_untracked())
                            >
                                <Icon name="download"/>
                                {format!("Download {n}")}
                            </button>
                        }
                    })}
            </div>
            <pre class="overflow-x-auto p-3 font-mono text-sm whitespace-pre">{move || text.get()}</pre>
        </div>
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: char,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[component]
pub fn DiffView(
    #[prop(into)] lines: Signal<Vec<DiffLine>>,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    view! {
        <div class="overflow-x-auto rounded-card border border-border font-mono text-sm" role="table" data-testid=(!testid.is_empty()).then_some(testid)>
            {move || {
                lines
                    .get()
                    .into_iter()
                    .map(|l| {
                        let class = match l.kind {
                            '+' => "grid grid-cols-[3rem_3rem_1.5rem_1fr] bg-success/10",
                            '-' => "grid grid-cols-[3rem_3rem_1.5rem_1fr] bg-danger/10",
                            '@' => "grid grid-cols-[3rem_3rem_1.5rem_1fr] bg-surface-2 text-text-muted",
                            _ => "grid grid-cols-[3rem_3rem_1.5rem_1fr]",
                        };
                        let num = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
                        view! {
                            <div class=class role="row">
                                <span class="pr-2 text-right text-text-muted select-none" role="cell">{num(l.old)}</span>
                                <span class="pr-2 text-right text-text-muted select-none" role="cell">{num(l.new)}</span>
                                <span class="text-center select-none" role="cell">{l.kind.to_string()}</span>
                                <span class="whitespace-pre" role="cell">{l.text}</span>
                            </div>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}
