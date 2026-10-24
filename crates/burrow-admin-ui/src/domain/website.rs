use burrow_admin_logic::check::check;
use burrow_admin_logic::tree::inner;
use burrow_admin_logic::web::{names, tls_of, website};
use burrow_admin_proto::schema::{Node, ObjectC, SemType, UnionC};
use burrow_admin_proto::serde_json::Value;
use burrow_config::WebSite;
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

fn tls_node(u: &UnionC) -> Option<Node> {
    u.branches.iter().find_map(|b| match &inner(b).sem {
        SemType::Object(o) => o
            .fields
            .iter()
            .find(|f| f.key == WebSite::TLS.key())
            .map(|f| f.node.clone()),
        _ => None,
    })
}

struct Choices {
    options: Vec<(String, String, bool)>,
    files: Option<ObjectC>,
    help: String,
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
    Choices {
        options,
        files,
        help: tls.map(|n| n.help.clone()).unwrap_or_default(),
    }
}

fn mode_of(web: Option<&Value>) -> String {
    match web.and_then(tls_of) {
        None => AUTO.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => FILES.to_string(),
    }
}

fn files_view(files: ObjectC, web: Memo<Option<Value>>) -> impl IntoView {
    move || {
        let current = web.get();
        let tls = current.as_ref().and_then(tls_of).cloned();
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
        (mode_of(current.as_ref()) == FILES).then(|| view! {
            <div class="rounded-input border border-border bg-surface-2 px-2.5 py-2" data-testid="website-files">
                {rows}
                <p class="text-xs text-text-muted">"Change the certificate files in server.yaml (burrow edit)."</p>
            </div>
        })
    }
}

pub(super) fn website_editor(fc: FieldCtx, u: UnionC, item: Node) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let value = Memo::new(move |_| e.value(&path));
    let items = Signal::derive(move || value.get().as_ref().map(names).unwrap_or_default());
    let mode = Signal::derive(move || mode_of(value.get().as_ref()));
    let one = item.clone();
    let check_one = Callback::new(move |t: String| check(&one, &Value::String(t)).err());
    let write = {
        let fc = fc.clone();
        move |list: Vec<String>, tls: Option<Value>| {
            fc.touched.set(true);
            let bad = list
                .iter()
                .find_map(|n| check(&item, &Value::String(n.clone())).err());
            e.local_error(&fc.path, bad);
            let now = value.get_untracked();
            e.set(&fc.path, website(now.as_ref(), &list, tls));
        }
    };
    let on_names = {
        let write = write.clone();
        move |list: Vec<String>| {
            let tls = value.get_untracked().as_ref().and_then(tls_of).cloned();
            write(list, tls)
        }
    };
    let on_mode = move |m: String| {
        let list = items.get_untracked();
        if !list.is_empty() && m != FILES {
            write(list, (m != AUTO).then(|| Value::String(m)));
        }
    };
    let Choices {
        options,
        files,
        help,
    } = choices(tls_node(&u).as_ref());
    let off = Signal::derive(move || {
        if mode.get() == FILES {
            Vec::new()
        } else {
            vec![FILES.to_string()]
        }
    });
    view! {
        <div id=fc.id.clone() class="flex flex-col gap-2" data-testid="website-editor">
            <ChipInput
                id=format!("{}-hosts", fc.id)
                items=items
                on_change=on_names
                check=check_one
                placeholder="Type a name like nas.example.com and press Enter"
                empty_label=fc.node.none_label.clone().unwrap_or_default()
            />
            <div class="flex flex-wrap items-center gap-x-3 gap-y-1">
                <span class="text-[13px] font-medium">"HTTPS"</span>
                <Segmented name=format!("{}-tls", fc.id) options=options value=mode label="HTTPS" off=off on_change=on_mode/>
            </div>
            {files.map(|f| files_view(f, value))}
            <p class="text-xs text-text-muted">{help}</p>
        </div>
    }
    .into_any()
}
