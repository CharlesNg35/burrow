use burrow_admin_logic::check::check;
use burrow_admin_logic::tree::inner;
use burrow_admin_logic::web::{names, passes_through, website};
use burrow_admin_proto::schema::{Node, SemType, UnionC};
use burrow_admin_proto::serde_json::Value;
use burrow_config::WebSite;
use burrow_ui_kit::{ChipInput, Segmented};
use leptos::prelude::*;

use crate::store::store;

use super::FieldCtx;

const PASS: &str = "passthrough";
const OWN: &str = "own";

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

fn tls_options(tls: Option<&Node>) -> (Vec<(String, String, bool)>, String) {
    let pass = tls
        .and_then(|n| match &inner(n).sem {
            SemType::Enum(e) => e.options.first().map(|o| o.label.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "Passthrough".into());
    let own = tls
        .and_then(|n| n.none_label.clone().or_else(|| inner(n).none_label.clone()))
        .unwrap_or_else(|| "burrow handles HTTPS".into());
    let help = tls.map(|n| n.help.clone()).unwrap_or_default();
    (
        vec![
            (PASS.to_string(), pass, false),
            (OWN.to_string(), own, false),
        ],
        help,
    )
}

pub(super) fn website_editor(fc: FieldCtx, u: UnionC, item: Node) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let value = Memo::new(move |_| e.value(&path));
    let items = Signal::derive(move || value.get().as_ref().map(names).unwrap_or_default());
    let mode = Signal::derive(move || {
        if value.get().as_ref().is_some_and(passes_through) {
            PASS.to_string()
        } else {
            OWN.to_string()
        }
    });
    let one = item.clone();
    let check_one = Callback::new(move |t: String| check(&one, &Value::String(t)).err());
    let write = {
        let fc = fc.clone();
        move |list: Vec<String>, pass: bool| {
            fc.touched.set(true);
            let bad = list
                .iter()
                .find_map(|n| check(&item, &Value::String(n.clone())).err());
            e.local_error(&fc.path, bad);
            e.set(&fc.path, website(&list, pass));
        }
    };
    let on_names = {
        let write = write.clone();
        move |list: Vec<String>| write(list, mode.get_untracked() == PASS)
    };
    let on_mode = move |m: String| {
        let list = items.get_untracked();
        if !list.is_empty() {
            write(list, m == PASS);
        }
    };
    let (options, help) = tls_options(tls_node(&u).as_ref());
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
                <span class="text-[13px] font-medium">"TLS"</span>
                <Segmented name=format!("{}-tls", fc.id) options=options value=mode label="TLS" on_change=on_mode/>
            </div>
            <p class="text-xs text-text-muted">{help}</p>
        </div>
    }
    .into_any()
}
