use burrow_admin_proto::schema::{Field, Node};
use burrow_admin_proto::serde_json::Value;

pub const KEY_PREFIX: &str = "burrow-admin.more";

pub fn is_set(node: &Node, value: Option<&Value>) -> bool {
    let empty = match value {
        None | Some(Value::Null) => true,
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        Some(Value::String(s)) => s.is_empty(),
        Some(_) => false,
    };
    match (&node.default, value) {
        (Some(_), _) if empty => false,
        (Some(d), Some(v)) => d != v,
        _ => !empty,
    }
}

pub fn split<'a>(fields: &[&'a Field], all_advanced: bool) -> (Vec<&'a Field>, Vec<&'a Field>) {
    fields
        .iter()
        .copied()
        .partition(|f| !(all_advanced || f.node.advanced))
}

pub fn summary(set: usize, total: usize) -> String {
    match (set, total - set.min(total)) {
        (0, 1) => "More settings · 1 at its default".into(),
        (0, n) => format!("More settings · {n} at their defaults"),
        (n, _) => format!("More settings · {n} set"),
    }
}

pub fn storage_key(page: &str, section: &[&str]) -> String {
    let parts: Vec<&str> = section
        .iter()
        .copied()
        .filter(|s| !s.is_empty() && *s != "*")
        .collect();
    let section = if parts.is_empty() {
        "general".to_string()
    } else {
        parts.join(".")
    };
    format!("{KEY_PREFIX}.{page}.{section}")
}

pub fn matches(field: &Field, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    !q.is_empty()
        && (field.node.label.to_lowercase().contains(&q)
            || field.key.to_lowercase().contains(&q)
            || field.key.replace('_', " ").contains(&q))
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    fn field(key: &str, label: &str, advanced: bool, default: Option<Value>) -> Field {
        Field {
            key: key.into(),
            node: Node {
                label: label.into(),
                advanced,
                default,
                ..Node::default()
            },
        }
    }

    #[test]
    fn a_field_is_set_when_it_differs_from_its_default_or_has_a_value_without_one() {
        let plain = field("deny_from", "Deny from", true, None).node;
        assert!(!is_set(&plain, None));
        assert!(!is_set(&plain, Some(&Value::Null)));
        assert!(!is_set(&plain, Some(&json!([]))));
        assert!(!is_set(&plain, Some(&json!({}))));
        assert!(is_set(&plain, Some(&json!(["198.51.100.7"]))));
        assert!(is_set(&plain, Some(&json!(false))));
        let with = field("mode", "Mode", true, Some(json!("failover"))).node;
        assert!(!is_set(&with, None));
        assert!(!is_set(&with, Some(&json!("failover"))));
        assert!(is_set(&with, Some(&json!("balance"))));
    }

    #[test]
    fn the_split_keeps_the_order_and_puts_advanced_fields_last() {
        let (a, b, c) = (
            field("visitors", "Visitors", false, None),
            field("deny_from", "Deny from", true, None),
            field("allow_from", "Allow from", false, None),
        );
        let all = [&a, &b, &c];
        let (essential, advanced) = split(&all, false);
        let keys = |v: &[&Field]| v.iter().map(|f| f.key.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&essential), ["visitors", "allow_from"]);
        assert_eq!(keys(&advanced), ["deny_from"]);
        let (essential, advanced) = split(&all, true);
        assert!(essential.is_empty());
        assert_eq!(keys(&advanced), ["visitors", "deny_from", "allow_from"]);
    }

    #[test]
    fn the_summary_says_what_is_inside() {
        assert_eq!(summary(0, 3), "More settings · 3 at their defaults");
        assert_eq!(summary(0, 1), "More settings · 1 at its default");
        assert_eq!(summary(1, 3), "More settings · 1 set");
        assert_eq!(summary(3, 3), "More settings · 3 set");
    }

    #[test]
    fn the_remembered_choice_is_kept_per_page_and_section() {
        assert_eq!(
            storage_key("services", &["access"]),
            "burrow-admin.more.services.access"
        );
        assert_eq!(
            storage_key("settings", &["web", "letsencrypt", ""]),
            "burrow-admin.more.settings.web.letsencrypt"
        );
        assert_eq!(
            storage_key("services", &["health_check", "*", ""]),
            "burrow-admin.more.services.health_check"
        );
        assert_eq!(
            storage_key("settings", &[""]),
            "burrow-admin.more.settings.general"
        );
    }

    #[test]
    fn a_search_matches_the_label_or_the_key() {
        let f = field("deny_from", "Deny from", true, None);
        assert!(matches(&f, "deny f"));
        assert!(matches(&f, "DENY_FROM"));
        assert!(matches(&f, "deny from"));
        assert!(!matches(&f, ""));
        assert!(!matches(&f, "allow"));
    }
}
