use burrow_admin_logic::check::check;
use burrow_admin_logic::tree::inner;
use burrow_admin_logic::web::{names, tls_of, website};
use burrow_admin_proto::schema::{Node, ObjectC, SemType, UnionC};
use burrow_admin_proto::serde_json::Value;
use burrow_config::{Entry, ServerConfig, ServiceEntry, WebConfig, WebSite};
use burrow_syntax::web::{SingleLabel, host_name};
use burrow_ui_kit::{ChipInput, Segmented};
use leptos::prelude::*;

use crate::store::store;

use super::FieldCtx;

const AUTO: &str = "auto";
const FILES: &str = "files";
const OFF: &str = "off";
const FILES_LABEL: &str = "My own certificate";

pub(super) fn host_item(u: &UnionC) -> Option<Node> {
    u.branches
        .iter()
        .find(|b| matches!(inner(b).sem, SemType::HostName(_)))
        .cloned()
}

struct Choices {
    options: Vec<(String, String, bool)>,
    files: Option<ObjectC>,
}

fn choices(tls: Option<&Node>) -> Choices {
    let mut options = Vec::new();
    let mut files = None;
    if let Some(SemType::Union(u)) = tls.map(|n| &inner(n).sem) {
        for b in &u.branches {
            match &inner(b).sem {
                SemType::Enum(e) => options.extend(
                    e.options
                        .iter()
                        .map(|o| (o.value.clone(), o.label.clone(), false)),
                ),
                SemType::Object(o) => files = Some(o.clone()),
                _ => {}
            }
        }
    }
    if files.is_some() {
        options.push((FILES.to_string(), FILES_LABEL.to_string(), false));
    }
    Choices { options, files }
}

fn mode_of(tls: Option<&Value>) -> String {
    match tls.filter(|t| !t.is_null()) {
        None => AUTO.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => FILES.to_string(),
    }
}

fn files_view(fc: FieldCtx, files: ObjectC, tls: Memo<Option<Value>>) -> impl IntoView {
    move || {
        let mode = mode_of(tls.get().as_ref());
        (mode == FILES).then(|| {
            view! {
                <div class="mt-2 rounded-input border border-border bg-surface-2 px-2.5 py-2" data-testid="website-files">
                    {crate::uploads::certificate_source(&fc, &files)}
                </div>
            }
        })
    }
}

pub(super) fn is_site_tls(fc: &FieldCtx) -> bool {
    matches!(fc.spath.as_slice(), [s, _, w, t] if s == ServiceEntry::SECTION && w == ServiceEntry::WEB.key() && t == WebSite::TLS.key())
}

pub(super) fn tls_editor(fc: FieldCtx) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let value = Memo::new(move |_| e.value(&path));
    let mode = Signal::derive(move || mode_of(value.get().as_ref()));
    let Choices { options, files } = choices(Some(&fc.node));
    let p2 = fc.path.clone();
    let touched = fc.touched;
    let on_mode = move |m: String| {
        touched.set(true);
        let v = match m.as_str() {
            AUTO => None,
            FILES if mode_of(e.value(&p2).as_ref()) == FILES => return,
            FILES => Some(Value::Object(Default::default())),
            _ => Some(Value::String(m)),
        };
        e.set(&p2, v);
    };
    let fc2 = fc.clone();
    view! {
        <div id=fc.id.clone() data-testid="website-tls">
            <Segmented name=format!("{}-choice", fc.id) options=options value=mode label=fc.node.label.clone() on_change=on_mode/>
            {files.map(|f| files_view(fc2, f, value))}
        </div>
    }
    .into_any()
}

fn web_off() -> impl IntoView {
    let open = move |_| {
        crate::route::go(crate::route::Route::Settings);
        request_animation_frame(|| {
            let card = format!("[data-schema='{}']", ServerConfig::WEB.key());
            if let Ok(Some(el)) = document().query_selector(&card) {
                el.scroll_into_view();
            }
        });
    };
    view! {
        <p class="mb-2 text-[13px] text-text-muted" data-testid="web-off">
            "The web proxy is off, so names here lead nowhere yet. "
            <button type="button" class="cursor-pointer font-semibold text-accent-ink hover:underline" data-testid="web-off-link" on:click=open>"Turn it on in Settings"</button>
        </p>
    }
}

fn name_problem(item: &Node, text: &str, plain: bool) -> Option<String> {
    if plain && host_name(text, SingleLabel::Allowed).is_ok() {
        return None;
    }
    check(item, &Value::String(text.to_string())).err()
}

pub(super) fn website_editor(fc: FieldCtx, item: Node) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let value = Memo::new(move |_| e.value(&path));
    let items = Signal::derive(move || value.get().as_ref().map(names).unwrap_or_default());
    let plain = Memo::new(move |_| {
        let https = e.draft.with(|d| {
            ServerConfig::WEB
                .get(d.draft())
                .is_some_and(|w| WebConfig::HTTPS.get(w).is_some_and(|h| !h.is_null()))
        });
        let tls = value.get().as_ref().and_then(tls_of).cloned();
        match tls {
            Some(t) => t == Value::String(OFF.into()),
            None => !https,
        }
    });
    let one = item.clone();
    let check_one = Callback::new(move |t: String| name_problem(&one, &t, plain.get_untracked()));
    let on_names = {
        let fc = fc.clone();
        move |list: Vec<String>| {
            fc.touched.set(true);
            let bad = list
                .iter()
                .find_map(|n| name_problem(&item, n, plain.get_untracked()));
            e.local_error(&fc.path, bad);
            let now = value.get_untracked();
            let tls = now.as_ref().and_then(tls_of).cloned();
            e.set(&fc.path, website(now.as_ref(), &list, tls));
        }
    };
    let off = Memo::new(move |_| {
        e.draft
            .with(|d| ServerConfig::WEB.get(d.draft()).is_none_or(Value::is_null))
    });
    view! {
        <div id=fc.id.clone() data-testid="website-editor">
            {move || off.get().then(web_off)}
            <ChipInput
                id=format!("{}-hosts", fc.id)
                items=items
                on_change=on_names
                check=check_one
                placeholder="Type a name like nas.example.com and press Enter"
                empty_label=fc.node.none_label.clone().unwrap_or_default()
            />
        </div>
    }
    .into_any()
}
