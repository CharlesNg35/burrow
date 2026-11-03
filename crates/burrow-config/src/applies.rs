use burrow_admin_proto::schema::{Lookup, Relation, SemType, decide};
use burrow_admin_proto::serde_json::{Value, json};

use crate::admin_schema::AdminSchema;
use crate::model::{TlsMode, WebSite};

#[derive(Debug, Clone, PartialEq)]
pub enum OnPassthrough {
    Refused,
    Fixed(Value),
}

pub fn on_passthrough() -> Vec<(String, OnPassthrough)> {
    let tls = |p: &str| {
        if p == WebSite::TLS.key() {
            Lookup::Value(json!(TlsMode::Passthrough.wire()))
        } else {
            Lookup::Unknown
        }
    };
    let SemType::Object(site) = WebSite::node().sem else {
        return Vec::new();
    };
    site.fields
        .into_iter()
        .filter_map(|f| {
            let rule = f.node.relations.iter().find_map(|r| match r {
                Relation::FixedIf { cond, value, .. } if decide(cond, &tls) == Some(true) => {
                    Some(OnPassthrough::Fixed(value.clone()))
                }
                Relation::EnabledIf { cond, .. } if decide(cond, &tls) == Some(false) => {
                    Some(OnPassthrough::Refused)
                }
                _ => None,
            })?;
            Some((f.key, rule))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_passthrough_site_keeps_its_names_and_https_redirect_stays_on() {
        let rules = on_passthrough();
        let rule = |k: &str| rules.iter().find(|(key, _)| key == k).map(|(_, r)| r);
        assert_eq!(
            rule(WebSite::HTTPS_REDIRECT.key()),
            Some(&OnPassthrough::Fixed(json!(true)))
        );
        assert_eq!(
            rule(WebSite::CHALLENGE.key()),
            Some(&OnPassthrough::Refused)
        );
        assert_eq!(rule(WebSite::APP_PATH.key()), Some(&OnPassthrough::Refused));
        assert_eq!(rule(WebSite::HOSTS.key()), None);
        assert_eq!(rule(WebSite::TLS.key()), None);
        assert_eq!(rules.len(), 22, "{rules:?}");
    }
}
