use burrow_admin_logic::grant::{IN_THE_FILE, would_grant};
use burrow_admin_proto::serde_json::{Map, Value};
use burrow_admin_proto::value::get;
use leptos::prelude::*;

use crate::store::store;

pub fn refuses_in(
    root: &burrow_admin_proto::schema::Node,
    draft: &burrow_admin_logic::changeset::Changeset,
    path: &[String],
    value: Option<&Value>,
) -> bool {
    let gated = (3..=path.len()).any(|n| root.at(&path[..n]).is_some_and(|x| x.reduce_only));
    if !gated {
        return false;
    }
    let entity = &path[..2];
    let mut candidate = get(draft.draft(), entity)
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    if !burrow_admin_proto::value::put(&mut candidate, &path[2..], value.cloned()) {
        return true;
    }
    would_grant(get(draft.base(), entity), &candidate)
}

pub fn refuses_text_in(
    root: &burrow_admin_proto::schema::Node,
    draft: &burrow_admin_logic::changeset::Changeset,
    path: &[String],
    text: Option<&str>,
) -> bool {
    let value = match text.map(burrow_admin_proto::serde_json::from_str::<Value>) {
        None => None,
        Some(Ok(v)) => Some(v),
        Some(Err(_)) => {
            return (3..=path.len()).any(|n| root.at(&path[..n]).is_some_and(|x| x.reduce_only));
        }
    };
    refuses_in(root, draft, path, value.as_ref())
}

pub fn refuses_text(path: &[String], text: Option<&str>) -> bool {
    let Some(s) = crate::store::try_store() else {
        return false;
    };
    s.schema.with_untracked(|sc| {
        sc.as_ref().is_some_and(|sc| {
            s.edit
                .draft
                .with_untracked(|d| refuses_text_in(&sc.root, d, path, text))
        })
    })
}

pub fn refuses(path: &[String], value: Option<&Value>) -> bool {
    let Some(s) = crate::store::try_store() else {
        return false;
    };
    s.schema.with_untracked(|sc| {
        sc.as_ref().is_some_and(|sc| {
            s.edit
                .draft
                .with_untracked(|d| refuses_in(&sc.root, d, path, value))
        })
    })
}

pub fn refused_line(path: Vec<String>) -> impl IntoView {
    let e = store().edit;
    move || e.refused.with(|r| r.contains(&path)).then(in_the_file)
}

pub fn in_the_file() -> impl IntoView {
    view! { <burrow_ui_kit::LockLine text=IN_THE_FILE testid="grant-lock" class="mt-1.5"/> }
}

#[cfg(test)]
mod tests {
    use burrow_admin_logic::changeset::Changeset;
    use burrow_admin_proto::schema::Schema;
    use burrow_admin_proto::serde_json::json;
    use burrow_config::{ClientEntry, VisitorEntry};

    use super::*;

    #[test]
    fn the_gate_refuses_a_grant_on_a_reduce_only_field_of_any_type() {
        let schema: Schema = burrow_admin_proto::serde_json::from_str(include_str!(
            "../../burrow-admin-proto/tests/fixtures/v0.1/schema.json"
        ))
        .unwrap();
        let secret = json!({"$secret": {"fp": "aaaa0001", "src": "literal"}});
        let draft = Changeset::new(json!({
            "clients": {"home": {"secret": secret.clone()}},
            "visitors": {"lap": {"secret": secret.clone(), "admin": "read", "admin_from": ["10.0.0.0/8"]}}
        }));
        let refuses = |path: Vec<String>, v: Option<Value>| {
            refuses_in(&schema.root, &draft, &path, v.as_ref())
        };
        assert!(refuses(VisitorEntry::ADMIN.of("lap"), Some(json!("write"))));
        assert!(!refuses(VisitorEntry::ADMIN.of("lap"), None));
        assert!(refuses(VisitorEntry::ADMIN_FROM.of("lap"), Some(json!([]))));
        assert!(refuses(
            VisitorEntry::ADMIN_FROM.of("lap"),
            Some(json!(["11.0.0.0/8"]))
        ));
        assert!(!refuses(
            VisitorEntry::ADMIN_FROM.of("lap"),
            Some(json!(["10.1.0.0/16"]))
        ));
        let added = json!([
            secret,
            burrow_admin_logic::changeset::new_secret("h", "bbbb0002")
        ]);
        assert!(refuses(VisitorEntry::SECRET.of("lap"), Some(added.clone())));
        assert!(!refuses(
            VisitorEntry::MAX_CONNECTIONS.of("lap"),
            Some(json!(5))
        ));
        assert!(!refuses(ClientEntry::SECRET.of("home"), Some(added)));
        let mut item = VisitorEntry::ADMIN_FROM.of("lap");
        item.push("0".into());
        assert!(
            refuses(item, Some(json!("0.0.0.0/0"))),
            "a single list item is gated too"
        );
        let text =
            |path: Vec<String>, t: &str| refuses_text_in(&schema.root, &draft, &path, Some(t));
        assert!(text(VisitorEntry::ADMIN.of("lap"), "\"write\""));
        assert!(!text(VisitorEntry::ADMIN.of("lap"), "\"read\""));
        assert!(text(VisitorEntry::ADMIN_FROM.of("lap"), "[\"0.0.0.0/0\"]"));
        assert!(!text(
            VisitorEntry::ADMIN_FROM.of("lap"),
            "[\"10.1.0.0/16\"]"
        ));
        assert!(
            text(VisitorEntry::ADMIN_FROM.of("lap"), "- 0.0.0.0/0"),
            "text the UI cannot read is refused on a reduce-only field"
        );
        assert!(!text(VisitorEntry::MAX_CONNECTIONS.of("lap"), "not json"));
        assert!(!refuses_text_in(
            &schema.root,
            &draft,
            &VisitorEntry::ADMIN.of("lap"),
            None
        ));
    }
}
