use burrow_admin_logic::tree::inner;
use burrow_admin_proto::schema::{EnumC, Node, SemType, UnionC};
use burrow_admin_proto::serde_json::Value;
use burrow_ui_kit::{Select, TextInput};
use leptos::prelude::*;

use crate::store::store;

use super::FieldCtx;

const UNSET: &str = "";
const OTHER: &str = "\u{1}other";

pub(super) fn enum_or_text(u: &UnionC) -> Option<(EnumC, Node)> {
    let [a, b] = u.branches.as_slice() else {
        return None;
    };
    match (&inner(a).sem, &inner(b).sem) {
        (SemType::Enum(c), SemType::Text(_)) => Some((c.clone(), b.clone())),
        (SemType::Text(_), SemType::Enum(c)) => Some((c.clone(), a.clone())),
        _ => None,
    }
}

pub(super) fn enum_or_text_editor(fc: FieldCtx, c: EnumC, text: Node) -> AnyView {
    let e = store().edit;
    let path = fc.path.clone();
    let now = Memo::new(move |_| e.value(&path));
    let named: Vec<String> = c.options.iter().map(|o| o.value.clone()).collect();
    let other = RwSignal::new(false);
    let mode = {
        let named = named.clone();
        Memo::new(move |_| match now.get() {
            Some(Value::String(s)) if named.contains(&s) => s,
            Some(Value::String(s)) if !s.is_empty() => OTHER.to_string(),
            _ if other.get() => OTHER.to_string(),
            _ => UNSET.to_string(),
        })
    };
    let mut options: Vec<(String, String)> = Vec::new();
    if !fc.node.required {
        let none = fc
            .node
            .none_label
            .clone()
            .unwrap_or_else(|| "Not set".into());
        options.push((UNSET.to_string(), none));
    }
    options.extend(c.options.iter().map(|o| (o.value.clone(), o.label.clone())));
    let other_label = if text.label.is_empty() {
        "Another address".to_string()
    } else {
        text.label.clone()
    };
    options.push((OTHER.to_string(), other_label.clone()));
    let p2 = fc.path.clone();
    let on_change = Callback::new(move |v: String| {
        e.local_error(&p2, None);
        other.set(v == OTHER);
        match v.as_str() {
            OTHER => {
                if now.with_untracked(|n| matches!(n, Some(Value::String(s)) if named.contains(s)))
                {
                    e.set(&p2, None);
                }
            }
            UNSET => e.set(&p2, None),
            _ => e.set(&p2, Some(Value::String(v))),
        }
    });
    let p3 = fc.path.clone();
    let touched = fc.touched;
    let typed = Signal::derive(move || match now.get() {
        Some(Value::String(s)) if mode.get() == OTHER => s,
        _ => String::new(),
    });
    let id = fc.id.clone();
    let input = move || {
        (mode.get() == OTHER).then(|| {
            let p3 = p3.clone();
            view! {
                <div class="mt-2">
                    <TextInput
                        id=format!("{id}-text")
                        value=typed
                        on_input=move |t: String| {
                            let t = t.trim().to_string();
                            e.set(&p3, (!t.is_empty()).then_some(Value::String(t)));
                        }
                        on_blur=move |_| { let _ = touched.try_set(true); }
                        placeholder="https://ca.example.com/acme/directory"
                        label=other_label.clone()
                        mono=true
                    />
                </div>
            }
        })
    };
    view! {
        <div data-editor="enum-or-text">
            <div class="max-w-80">
                <Select id=fc.id.clone() options=options value=Signal::derive(move || mode.get()) on_change=on_change label=fc.node.label.clone()/>
            </div>
            {input}
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_config::admin_schema::AdminSchema;

    #[test]
    fn a_fixed_list_or_free_text_is_one_select_with_an_address() {
        let node = burrow_config::Directory::node();
        let SemType::Union(u) = &inner(&node).sem else {
            panic!("{node:?}")
        };
        let (c, _) = enum_or_text(u).expect("the directory is a list or an address");
        let values: Vec<&str> = c.options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, ["production", "staging"]);
        let site = burrow_config::SiteTls::node();
        let SemType::Union(u) = &inner(&site).sem else {
            panic!("{site:?}")
        };
        assert!(
            enum_or_text(u).is_none(),
            "a list or files is not free text"
        );
    }
}
