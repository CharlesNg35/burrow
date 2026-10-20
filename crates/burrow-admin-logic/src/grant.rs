use burrow_admin_proto::msg::Level;
use burrow_admin_proto::serde_json::{self, Value};
use burrow_admin_proto::value::as_secret;
use burrow_syntax::grant::{Power, grants};

use crate::changeset::as_new_secret;

pub const IN_THE_FILE: &str = "Admin access is granted in the config file (burrow edit)";

pub const LEVEL: &str = "admin";
pub const SECRETS: &str = "secret";
pub const FROM: &str = "admin_from";
pub const FIELDS: [&str; 3] = [LEVEL, SECRETS, FROM];

pub fn power_of(visitor: &Value) -> Power<Level, Value> {
    let level = visitor
        .get(LEVEL)
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let secrets = match visitor.get(SECRETS) {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Null) | None => Vec::new(),
        Some(one) => vec![one.clone()],
    }
    .into_iter()
    .filter(|s| as_secret(s).is_some() || as_new_secret(s).is_some())
    .collect();
    let from = visitor
        .get(FROM)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .filter_map(|t| t.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    Power {
        level,
        secrets,
        from,
    }
}

pub fn would_grant(base: Option<&Value>, candidate: &Value) -> bool {
    let old = base.map(power_of);
    !grants(old.as_ref(), &power_of(candidate)).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changeset::new_secret;
    use crate::testing::secret;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn a_rename_is_a_new_visitor() {
        let renamed = json!({"secret": secret("aaaa0001"), "admin": "write"});
        assert!(would_grant(None, &renamed));
        assert!(!would_grant(None, &json!({"secret": secret("aaaa0001")})));
    }

    #[test]
    fn a_visitor_value_gives_its_admin_power() {
        let v = json!({
            "secret": [secret("aaaa0001"), new_secret("s1", "bbbb0002")],
            "admin": "read",
            "admin_from": ["10.0.0.0/8", "not a cidr", "::ffff:192.0.2.1"]
        });
        let p = power_of(&v);
        assert_eq!(p.level, Some(Level::Read));
        assert_eq!(p.secrets.len(), 2);
        assert_eq!(
            p.from,
            vec!["10.0.0.0/8".parse().unwrap(), "192.0.2.1".parse().unwrap()]
        );
        let one = power_of(&json!({"secret": secret("aaaa0001")}));
        assert_eq!(one.level, None);
        assert_eq!(one.secrets, vec![secret("aaaa0001")]);
        assert!(one.from.is_empty());
    }

    #[test]
    fn a_candidate_value_is_judged_against_the_base() {
        let base =
            json!({"secret": secret("aaaa0001"), "admin": "write", "admin_from": ["10.0.0.0/8"]});
        let with = |k: &str, v: Value| {
            let mut c = base.clone();
            c[k] = v;
            c
        };
        assert!(!would_grant(Some(&base), &base));
        assert!(!would_grant(Some(&base), &with("admin", json!("read"))));
        assert!(!would_grant(
            Some(&base),
            &with("admin_from", json!(["10.1.0.0/16"]))
        ));
        assert!(would_grant(Some(&base), &with("admin_from", json!([]))));
        assert!(would_grant(
            Some(&base),
            &with("admin_from", json!(["11.0.0.0/8"]))
        ));
        assert!(would_grant(
            Some(&base),
            &with(
                "secret",
                json!([secret("aaaa0001"), new_secret("s1", "cccc0003")])
            )
        ));
        let read = with("admin", json!("read"));
        assert!(would_grant(Some(&read), &with("admin", json!("write"))));
        let plain = json!({"secret": secret("aaaa0001")});
        assert!(would_grant(Some(&plain), &with("admin", json!("read"))));
        assert!(would_grant(None, &with("admin", json!("read"))));
    }
}
