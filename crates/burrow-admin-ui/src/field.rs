use burrow_admin_logic::check::{check_in, refs_exist};
use burrow_admin_logic::tree::{inner, names};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{Entity, Node, SemType};
use burrow_admin_proto::serde_json::Value;
use burrow_ui_kit::{Chip, Tone, chip_class};
use leptos::prelude::*;

use crate::route::{Link, Route};
use crate::store::store;

const MAX_TEXT: usize = 4096;

#[derive(Clone)]
pub struct FieldCtx {
    pub node: Node,
    pub path: Path,
    pub spath: Path,
    pub entity: Path,
    pub id: String,
    pub msg_id: String,
    pub touched: RwSignal<bool>,
    pub inherited: Option<Value>,
}

pub fn names_of(entity: &Entity) -> Vec<String> {
    let doc = store().edit.doc();
    names(&doc, &burrow_config::entity_path(entity))
}

pub fn set_checked(fc: &FieldCtx, node: &Node, v: Option<Value>) {
    let e = store().edit;
    if crate::grants::refuses(&fc.path, v.as_ref()) {
        e.refuse(&fc.path, true);
        return;
    }
    let problem = match &v {
        None if node.required && node.default.is_none() => {
            Some(burrow_admin_logic::wording::required(&node.label))
        }
        None => None,
        Some(x) => check_in(node, x, &e.doc())
            .err()
            .or_else(|| refs_exist(node, x, &ref_names(node)).err()),
    };
    e.local_error(&fc.path, problem);
    e.set(&fc.path, v);
}

fn ref_names(node: &Node) -> Vec<String> {
    match &inner(node).sem {
        SemType::Ref(r) => names_of(&r.entity),
        SemType::RefList(r) => names_of(&r.entity),
        SemType::RefEntries(r) => names_of(&r.entity),
        _ => Vec::new(),
    }
}

#[derive(Clone)]
pub struct Ctx {
    pub doc: Value,
    pub path: Vec<String>,
}

pub(crate) fn cap(s: String) -> String {
    if s.len() <= MAX_TEXT {
        return s;
    }
    let mut end = MAX_TEXT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

pub fn plain(v: &Value) -> String {
    cap(match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

pub(crate) fn ref_link(entity: &Entity, name: &str, reserved: &[String], ctx: &Ctx) -> AnyView {
    if reserved.iter().any(|r| r == name) {
        let to = Route::Settings;
        let local = name.to_string();
        return view! {
            <Link to=to class=chip_class(Tone::Accent, true)>
                <span data-local=local>{burrow_admin_logic::wording::THIS_SERVER}</span>
            </Link>
        }
        .into_any();
    }
    let mut at = burrow_config::entity_path(entity);
    at.push(name.to_string());
    let exists = burrow_admin_proto::value::get(&ctx.doc, &at).is_some();
    let name = name.to_string();
    if exists {
        let to = match entity {
            Entity::Login => Route::Settings,
            e => Route::Detail(e.section().to_string(), name.clone()),
        };
        view! { <Link to=to class=chip_class(Tone::Accent, true)>{name}</Link> }.into_any()
    } else {
        view! { <Chip text=name mono=true tone=Tone::Danger/> }.into_any()
    }
}

pub fn problem_at(root: &Node, spath: &[String], v: Option<&Value>, doc: &Value) -> Option<String> {
    let node = root.at(spath)?;
    match v.filter(|v| !v.is_null()) {
        None if node.required && node.default.is_none() => {
            Some(burrow_admin_logic::wording::required(&node.label))
        }
        None => None,
        Some(x) => check_in(node, x, doc).err(),
    }
}

pub fn concrete(spath: &[String], name: &str) -> Path {
    spath
        .iter()
        .map(|s| {
            if s == "*" {
                name.to_string()
            } else {
                s.clone()
            }
        })
        .collect()
}
