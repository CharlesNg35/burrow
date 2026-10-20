use burrow_admin_proto::msg::Level;
use burrow_admin_proto::serde_json::{self, Value};
use burrow_admin_proto::value::as_secret;
use burrow_syntax::cidr::{Cidr, Spans, within};

use crate::changeset::as_new_secret;

pub const IN_THE_FILE: &str = "Admin access is granted in the config file (burrow edit)";

pub const LEVEL: &str = "admin";
pub const SECRETS: &str = "secret";
pub const FROM: &str = "admin_from";
pub const FIELDS: [&str; 3] = [LEVEL, SECRETS, FROM];

pub const CODES: [&str; 4] = [
    "admin-grant",
    "admin-upgrade",
    "admin-secret-added",
    "admin-from-widened",
];

pub fn is_grant_code(code: &str) -> bool {
    CODES.contains(&code)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Power<S> {
    pub level: Option<Level>,
    pub secrets: Vec<S>,
    pub from: Vec<Cidr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Grant<'a, S> {
    Admin(Level),
    Upgrade,
    SecretAdded(&'a S),
    FromWidened(Option<Cidr>),
}

impl<S> Grant<'_, S> {
    pub fn code(&self) -> &'static str {
        match self {
            Grant::Admin(_) => CODES[0],
            Grant::Upgrade => CODES[1],
            Grant::SecretAdded(_) => CODES[2],
            Grant::FromWidened(_) => CODES[3],
        }
    }
}

pub fn grants<'a, S: PartialEq>(old: Option<&Power<S>>, new: &'a Power<S>) -> Vec<Grant<'a, S>> {
    let Some(level) = new.level else {
        return Vec::new();
    };
    let Some(old) = old.filter(|o| o.level.is_some()) else {
        return vec![Grant::Admin(level)];
    };
    let mut out = Vec::new();
    if old.level < new.level {
        out.push(Grant::Upgrade);
    }
    out.extend(
        new.secrets
            .iter()
            .filter(|s| !old.secrets.contains(s))
            .map(Grant::SecretAdded),
    );
    if !within(&new.from, &old.from) {
        if new.from.is_empty() {
            out.push(Grant::FromWidened(None));
        } else {
            let spans = Spans::of(&old.from);
            out.extend(
                new.from
                    .iter()
                    .filter(|c| !spans.covers(c))
                    .map(|c| Grant::FromWidened(Some(*c))),
            );
        }
    }
    out
}

pub fn power_of(visitor: &Value) -> Power<Value> {
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

    fn power(level: Option<Level>, secrets: &[&'static str], from: &[&str]) -> Power<&'static str> {
        Power {
            level,
            secrets: secrets.to_vec(),
            from: from.iter().map(|c| c.parse().unwrap()).collect(),
        }
    }

    fn codes(old: Option<&Power<&'static str>>, new: &Power<&'static str>) -> Vec<&'static str> {
        grants(old, new).iter().map(Grant::code).collect()
    }

    const R: Option<Level> = Some(Level::Read);
    const W: Option<Level> = Some(Level::Write);

    #[test]
    fn admin_for_a_visitor_without_it_is_a_grant() {
        let none = power(None, &["a"], &[]);
        assert_eq!(
            grants(Some(&none), &power(R, &["a"], &[])),
            vec![Grant::Admin(Level::Read)]
        );
        assert_eq!(
            grants(Some(&none), &power(W, &["a", "b"], &["10.0.0.0/8"])),
            vec![Grant::Admin(Level::Write)]
        );
        assert_eq!(
            grants(None, &power(W, &["a"], &[])),
            vec![Grant::Admin(Level::Write)]
        );
        assert!(grants(Some(&power(R, &["a"], &[])), &none).is_empty());
        assert!(grants(None, &power(None, &["a"], &[])).is_empty());
    }

    #[test]
    fn read_to_write_is_an_upgrade_and_write_to_read_is_not() {
        assert_eq!(
            codes(Some(&power(R, &["a"], &[])), &power(W, &["a"], &[])),
            ["admin-upgrade"]
        );
        assert!(codes(Some(&power(W, &["a"], &[])), &power(R, &["a"], &[])).is_empty());
        assert!(codes(Some(&power(W, &["a"], &[])), &power(W, &["a"], &[])).is_empty());
        assert!(codes(Some(&power(R, &["a"], &[])), &power(R, &["a"], &[])).is_empty());
    }

    #[test]
    fn a_new_secret_on_an_admin_is_a_grant_and_a_removed_one_is_not() {
        let old = power(W, &["a", "b"], &[]);
        assert_eq!(
            grants(Some(&old), &power(W, &["a", "b", "c"], &[])),
            vec![Grant::SecretAdded(&"c")]
        );
        assert_eq!(
            grants(Some(&old), &power(W, &["a", "c"], &[])),
            vec![Grant::SecretAdded(&"c")]
        );
        assert!(grants(Some(&old), &power(W, &["b"], &[])).is_empty());
        assert!(grants(Some(&old), &power(W, &["b", "a"], &[])).is_empty());
        assert!(grants(Some(&old), &power(None, &["c"], &[])).is_empty());
        assert!(
            grants(
                Some(&power(None, &["a"], &[])),
                &power(None, &["a", "c"], &[])
            )
            .is_empty()
        );
    }

    #[test]
    fn admin_from_may_narrow_but_not_widen() {
        let old = power(W, &["a"], &["10.0.0.0/8", "2001:db8::/32"]);
        let same = |from: &[&str]| codes(Some(&old), &power(W, &["a"], from));
        assert!(same(&["10.0.0.0/8"]).is_empty());
        assert!(same(&["10.1.0.0/16", "10.2.0.0/16", "2001:db8:1::/48"]).is_empty());
        assert!(same(&["10.9.8.7", "::ffff:10.1.2.3"]).is_empty());
        assert_eq!(same(&["10.0.0.0/8", "192.0.2.1"]), ["admin-from-widened"]);
        assert_eq!(same(&["10.0.0.0/7"]), ["admin-from-widened"]);
        assert_eq!(same(&["::/0"]), ["admin-from-widened"]);
        assert_eq!(same(&["10.0.0.0/8", "2001:db9::1"]), ["admin-from-widened"]);
        assert_eq!(
            grants(Some(&old), &power(W, &["a"], &[])),
            vec![Grant::FromWidened(None)]
        );
        assert_eq!(
            grants(
                Some(&old),
                &power(W, &["a"], &["10.0.0.0/8", "192.0.2.0/24"])
            ),
            vec![Grant::FromWidened(Some("192.0.2.0/24".parse().unwrap()))]
        );
        let split = power(W, &["a"], &["10.0.0.0/25", "10.0.0.128/25"]);
        assert!(codes(Some(&split), &power(W, &["a"], &["10.0.0.0/24"])).is_empty());
        let everywhere = power(W, &["a"], &[]);
        assert!(codes(Some(&everywhere), &power(W, &["a"], &["0.0.0.0/0", "::/0"])).is_empty());
        assert!(codes(Some(&everywhere), &power(W, &["a"], &["203.0.113.9"])).is_empty());
        let both = power(W, &["a"], &["0.0.0.0/0", "::/0"]);
        assert!(codes(Some(&both), &power(W, &["a"], &[])).is_empty());
        assert!(codes(Some(&old), &power(None, &["a"], &[])).is_empty());
    }

    #[test]
    fn every_kind_is_reported_once_per_cause() {
        let old = power(R, &["a"], &["10.0.0.0/8"]);
        assert_eq!(
            codes(
                Some(&old),
                &power(W, &["b"], &["192.0.2.1", "198.51.100.0/24"])
            ),
            [
                "admin-upgrade",
                "admin-secret-added",
                "admin-from-widened",
                "admin-from-widened"
            ]
        );
        assert!(CODES.iter().all(|c| is_grant_code(c)));
        assert!(!is_grant_code("self-demoted"));
    }

    #[test]
    fn a_rename_is_a_new_visitor() {
        let doc = json!({
            "visitors": {"ops2": {"secret": secret("aaaa0001"), "admin": "write"}}
        });
        let renamed = &doc["visitors"]["ops2"];
        assert!(would_grant(None, renamed));
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
        assert_eq!(p.level, R);
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
