use leptos::prelude::*;
use wasm_bindgen::JsCast;

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

#[component]
pub fn CodeBlock(
    #[prop(into)] text: Signal<String>,
    #[prop(optional, into)] file_name: Option<String>,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    let copied = RwSignal::new(false);
    let name = file_name.clone();
    let btn = "inline-flex h-[24px] cursor-pointer items-center gap-1.5 rounded-sm border border-white/15 bg-white/5 px-2 text-xs font-semibold text-code-text/85 hover:bg-white/10 hover:text-code-text";
    view! {
        <div class="relative overflow-hidden rounded-[10px] bg-code text-code-text" data-testid=(!testid.is_empty()).then_some(testid)>
            <div class="absolute top-2.5 right-2.5 flex gap-1.5">
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
            <pre class="overflow-x-auto px-4 py-3.5 pr-44 font-mono text-[12.5px] leading-[1.7] whitespace-pre">
                {move || {
                    let t = text.get();
                    let n = t.lines().count();
                    t.lines()
                        .enumerate()
                        .map(|(i, l)| view! { {code_line(l)}{(i + 1 < n).then_some("\n")} })
                        .collect_view()
                }}
            </pre>
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

fn row_class(kind: char) -> &'static str {
    match kind {
        '+' => "bg-diff-add",
        '-' => "bg-diff-del",
        '@' => "bg-accent-weak text-accent-ink",
        _ => "",
    }
}

fn sign(kind: char) -> (&'static str, &'static str) {
    match kind {
        '+' => ("+", "text-success"),
        '-' => ("−", "text-danger"),
        _ => ("", ""),
    }
}

#[component]
pub fn DiffView(
    #[prop(into)] lines: Signal<Vec<DiffLine>>,
    #[prop(optional, into)] split: Signal<bool>,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    let unified = move || {
        lines
            .get()
            .into_iter()
            .map(|l| {
                let num = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
                let (s, sc) = sign(l.kind);
                view! {
                    <div class=format!("grid grid-cols-[38px_38px_18px_1fr] {}", row_class(l.kind)) role="row">
                        <span class="pr-2 text-right text-text-faint select-none" role="cell">{num(l.old)}</span>
                        <span class="pr-2 text-right text-text-faint select-none" role="cell">{num(l.new)}</span>
                        <span class=format!("text-center select-none {sc}") role="cell">{s}</span>
                        <span class="pr-3 whitespace-pre" role="cell">{l.text}</span>
                    </div>
                }
            })
            .collect_view()
    };
    let side = move || {
        let all = lines.get();
        let mut rows: Vec<(Option<DiffLine>, Option<DiffLine>)> = Vec::new();
        let mut dels: Vec<DiffLine> = Vec::new();
        let mut adds: Vec<DiffLine> = Vec::new();
        let flush = |rows: &mut Vec<(Option<DiffLine>, Option<DiffLine>)>,
                     dels: &mut Vec<DiffLine>,
                     adds: &mut Vec<DiffLine>| {
            let n = dels.len().max(adds.len());
            let (d, a) = (std::mem::take(dels), std::mem::take(adds));
            let mut d = d.into_iter();
            let mut a = a.into_iter();
            for _ in 0..n {
                rows.push((d.next(), a.next()));
            }
        };
        for l in all {
            match l.kind {
                '-' => dels.push(l),
                '+' => adds.push(l),
                _ => {
                    flush(&mut rows, &mut dels, &mut adds);
                    rows.push((Some(l.clone()), Some(l)));
                }
            }
        }
        flush(&mut rows, &mut dels, &mut adds);
        rows.into_iter()
            .map(|(a, b)| {
                let half = |l: Option<DiffLine>, old: bool| match l {
                    Some(l) => {
                        let n = if old { l.old } else { l.new };
                        let (s, sc) = sign(l.kind);
                        view! {
                            <div class=format!("grid grid-cols-[38px_18px_1fr] {}", row_class(l.kind))>
                                <span class="pr-2 text-right text-text-faint select-none">{n.map(|n| n.to_string()).unwrap_or_default()}</span>
                                <span class=format!("text-center select-none {sc}")>{s}</span>
                                <span class="pr-3 whitespace-pre">{l.text}</span>
                            </div>
                        }
                        .into_any()
                    }
                    None => view! { <div></div> }.into_any(),
                };
                view! { <div class="grid grid-cols-2 divide-x divide-border" role="row">{half(a, true)}{half(b, false)}</div> }
            })
            .collect_view()
    };
    view! {
        <div
            class="overflow-x-auto rounded-card border border-border bg-surface font-mono text-[12.5px] leading-[1.8]"
            role="table"
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            {move || if split.get() { side().into_any() } else { unified().into_any() }}
        </div>
    }
}
