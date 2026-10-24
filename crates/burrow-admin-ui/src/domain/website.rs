use burrow_admin_logic::check::check;
use burrow_admin_logic::tree::inner;
use burrow_admin_logic::web::{names, tls_of, website};
use burrow_admin_proto::schema::{Node, ObjectC, SemType, UnionC};
use burrow_admin_proto::serde_json::Value;
use burrow_config::{Entry, ServiceEntry, WebSite};
use burrow_ui_kit::{ChipInput, Segmented};
use leptos::prelude::*;

use crate::store::store;

use super::FieldCtx;

const AUTO: &str = "auto";
const FILES: &str = "files";
const FILES_LABEL: &str = "My own certificate file";

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

fn files_view(files: ObjectC, tls: Memo<Option<Value>>) -> impl IntoView {
    move || {
        let tls = tls.get();
        let rows = files
            .fields
            .iter()
            .map(|f| {
                let shown = tls
                    .as_ref()
                    .and_then(|t| t.get(&f.key))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                view! {
                    <p class="flex flex-wrap gap-x-2 text-[13px]">
                        <span class="font-medium">{f.node.label.clone()}</span>
                        <span class="font-mono" data-locked="">{shown}</span>
                    </p>
                }
            })
            .collect_view();
        (mode_of(tls.as_ref()) == FILES).then(|| view! {
            <div class="mt-2 rounded-input border border-border bg-surface-2 px-2.5 py-2" data-testid="website-files">
                {rows}
                <p class="text-xs text-text-muted">"Change the certificate files in server.yaml (burrow edit)."</p>
            </div>
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
    let off = Signal::derive(move || {
        if mode.get() == FILES {
            Vec::new()
        } else {
            vec![FILES.to_string()]
        }
    });
    let p2 = fc.path.clone();
    let touched = fc.touched;
    let on_mode = move |m: String| {
        if m != FILES {
            touched.set(true);
            e.set(&p2, (m != AUTO).then(|| Value::String(m)));
        }
    };
    view! {
        <div id=fc.id.clone() data-testid="website-tls">
            <Segmented name=format!("{}-choice", fc.id) options=options value=mode label=fc.node.label.clone() off=off on_change=on_mode/>
            {files.map(|f| files_view(f, value))}
        </div>
    }
    .into_any()
}

pub(super) fn website_editor(fc: FieldCtx, item: Node) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let value = Memo::new(move |_| e.value(&path));
    let items = Signal::derive(move || value.get().as_ref().map(names).unwrap_or_default());
    let one = item.clone();
    let check_one = Callback::new(move |t: String| check(&one, &Value::String(t)).err());
    let on_names = {
        let fc = fc.clone();
        move |list: Vec<String>| {
            fc.touched.set(true);
            let bad = list
                .iter()
                .find_map(|n| check(&item, &Value::String(n.clone())).err());
            e.local_error(&fc.path, bad);
            let now = value.get_untracked();
            let tls = now.as_ref().and_then(tls_of).cloned();
            e.set(&fc.path, website(now.as_ref(), &list, tls));
        }
    };
    view! {
        <div id=fc.id.clone() data-testid="website-editor">
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
