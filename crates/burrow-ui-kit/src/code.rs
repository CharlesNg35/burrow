use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::icon::Icon;

pub fn copy_text(text: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.navigator().clipboard().write_text(text);
    }
}

pub fn download_text(name: &str, text: &str) {
    download(name, text, "text/plain");
}

pub fn download(name: &str, text: &str, mime: &str) {
    let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(text));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
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

fn code_line(line: &str) -> AnyView {
    let t = line.trim_start();
    if t.starts_with('#') {
        return view! { <span class="text-code-text/45">{line.to_string()}</span> }.into_any();
    }
    if let Some(rest) = line.strip_prefix("$ ") {
        return view! { <span><span class="text-code-text/45">"$ "</span>{rest.to_string()}</span> }.into_any();
    }
    let indent = line.len() - t.len();
    let (dash, body) = match t.strip_prefix("- ") {
        Some(b) => ("- ", b),
        None => ("", t),
    };
    if let Some((k, v)) = body.split_once(':')
        && !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && (v.is_empty() || v.starts_with(' '))
    {
        let pad = " ".repeat(indent);
        return view! {
            <span>{pad}{dash}<span class="text-[#8cc4f5]">{k.to_string()}</span>":"<span class="text-[#b5e3a8]">{v.to_string()}</span></span>
        }
        .into_any();
    }
    view! { <span>{line.to_string()}</span> }.into_any()
}

const CHUNK_LINES: usize = 200;
const LINE_PX: f64 = 21.25;

fn code_chunks(text: &str) -> AnyView {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= CHUNK_LINES {
        return chunk_lines(&lines);
    }
    lines
        .chunks(CHUNK_LINES)
        .map(|part| {
            let style = format!(
                "display:block;content-visibility:auto;contain-intrinsic-height:auto {}px;min-width:100%;width:max-content",
                part.len() as f64 * LINE_PX
            );
            view! { <span style=style data-code-chunk="">{chunk_lines(part)}</span> }
        })
        .collect_view()
        .into_any()
}

fn chunk_lines(lines: &[&str]) -> AnyView {
    let n = lines.len();
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| view! { {code_line(l)}{(i + 1 < n).then_some("\n")} })
        .collect_view()
        .into_any()
}

const SCROLL: &str = "overflow-auto max-h-[max(80vh,min(240px,100vh))] supports-[height:1dvh]:max-h-[max(80dvh,min(240px,100dvh))] px-4 py-3.5 font-mono max-sm:pt-2 sm:pr-44 text-[12.5px] leading-[1.7] whitespace-pre focus-visible:outline-offset-[-2px]";

#[component]
pub fn CodeBlock(
    #[prop(into)] text: Signal<String>,
    #[prop(optional, into)] file_name: Option<String>,
    #[prop(optional)] testid: &'static str,
    #[prop(optional)] fills: bool,
) -> impl IntoView {
    let copied = RwSignal::new(false);
    let name = file_name.clone();
    let region = file_name.unwrap_or_else(|| "Code".to_string());
    let btn = "inline-flex h-[24px] cursor-pointer items-center gap-1.5 rounded-sm border border-white/15 bg-white/5 px-2 text-xs font-semibold text-code-text/85 hover:bg-white/10 hover:text-code-text";
    view! {
        <div class="relative overflow-hidden rounded-[10px] bg-code text-code-text" data-testid=(!testid.is_empty()).then_some(testid) data-fill-part=fills.then_some("")>
            <div class="flex justify-end gap-1.5 px-2.5 pt-2.5 sm:absolute sm:top-2.5 sm:right-2.5 sm:p-0">
                <button
                    type="button"
                    class=btn
                    on:click=move |_| {
                        copy_text(&text.get_untracked());
                        copied.set(true);
                    }
                >
                    <Icon name="copy" small=true/>
                    {move || if copied.get() { "Copied" } else { "Copy" }}
                </button>
                {name.map(|n| {
                    view! {
                        <button type="button" class=btn aria-label=format!("Download {n}") on:click=move |_| download_text(&n, &text.get_untracked())>
                            <Icon name="download" small=true/>
                            "Download"
                        </button>
                    }
                })}
            </div>
            <pre
                class=SCROLL
                data-fill-scroll=fills.then_some("")
                tabindex="0"
                role="region"
                aria-label=region
            >
                {move || text.with(|t| code_chunks(t))}
            </pre>
        </div>
    }
}
