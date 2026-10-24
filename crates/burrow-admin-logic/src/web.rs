use burrow_admin_proto::serde_json::{Map, json};
use burrow_config::{ServiceEntry, TlsMode, WebSite};

use crate::Value;

pub fn hosts(svc: &Value) -> Vec<String> {
    ServiceEntry::WEB.get(svc).map(names).unwrap_or_default()
}

pub fn names(web: &Value) -> Vec<String> {
    let text = |v: &Value| v.as_str().map(str::to_string);
    match Some(web) {
        Some(Value::String(h)) => vec![h.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(text).collect(),
        Some(site @ Value::Object(_)) => WebSite::HOSTS
            .get(site)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(text).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn passthrough() -> Value {
    json!(TlsMode::Passthrough)
}

pub fn tls_of(web: &Value) -> Option<&Value> {
    WebSite::TLS.get(web).filter(|t| !t.is_null())
}

pub fn passes_through(web: &Value) -> bool {
    tls_of(web) == Some(&passthrough())
}

pub fn website(names: &[String], tls: Option<Value>) -> Option<Value> {
    let list = || Value::Array(names.iter().map(|n| Value::String(n.clone())).collect());
    match (names, tls) {
        ([], _) => None,
        (_, Some(tls)) => {
            let mut site = Map::new();
            site.insert(WebSite::HOSTS.key().into(), list());
            site.insert(WebSite::TLS.key().into(), tls);
            Some(Value::Object(site))
        }
        ([one], None) => Some(Value::String(one.clone())),
        (_, None) => Some(list()),
    }
}

pub fn exposure(svc: &Value) -> Option<String> {
    if let Some(b) = ServiceEntry::BIND.get(svc).filter(|b| !b.is_null()) {
        return Some(match b {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        });
    }
    let names = hosts(svc);
    (!names.is_empty()).then(|| names.join(", "))
}

pub fn is_public(svc: &Value) -> bool {
    exposure(svc).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn a_website_is_reached_by_its_names_in_all_three_forms() {
        assert_eq!(hosts(&json!({"web": "a.example.com"})), ["a.example.com"]);
        assert_eq!(
            hosts(&json!({"web": ["a.example.com", "b.example.com"]})),
            ["a.example.com", "b.example.com"]
        );
        let site = json!({"web": {"hosts": ["nas.example.com"], "tls": "passthrough"}});
        assert_eq!(hosts(&site), ["nas.example.com"]);
        assert_eq!(exposure(&site).as_deref(), Some("nas.example.com"));
        assert!(is_public(&site));
    }

    #[test]
    fn a_website_is_written_in_its_shortest_form() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(website(&n(&[]), Some(passthrough())), None);
        assert_eq!(
            website(&n(&["a.example.com"]), None),
            Some(json!("a.example.com"))
        );
        assert_eq!(
            website(&n(&["a.example.com", "b.example.com"]), None),
            Some(json!(["a.example.com", "b.example.com"]))
        );
        let off = website(&n(&["a.example.com"]), Some(json!("off"))).unwrap();
        assert_eq!(tls_of(&off), Some(&json!("off")));
        let files = json!({"cert": "c.pem", "key": "k.pem"});
        let own = website(&n(&["a.example.com"]), Some(files.clone())).unwrap();
        assert_eq!(tls_of(&own), Some(&files));
        let site = website(&n(&["a.example.com"]), Some(passthrough())).unwrap();
        assert_eq!(
            site,
            json!({"hosts": ["a.example.com"], "tls": "passthrough"})
        );
        assert!(passes_through(&site));
        assert!(!passes_through(&json!("a.example.com")));
        assert_eq!(names(&site), ["a.example.com"]);
    }

    #[test]
    fn a_bind_comes_first_and_nothing_means_private() {
        let both = json!({"bind": 7840, "web": "a.example.com"});
        assert_eq!(exposure(&both).as_deref(), Some("7840"));
        assert_eq!(exposure(&json!({"target": 22})), None);
        assert!(!is_public(&json!({"bind": null, "web": []})));
    }
}
