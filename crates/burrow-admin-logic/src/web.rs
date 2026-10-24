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

fn as_site(web: Option<&Value>) -> Map<String, Value> {
    match web {
        Some(Value::Object(m)) => m.clone(),
        Some(v @ (Value::String(_) | Value::Array(_))) => {
            let mut site = Map::new();
            let list = names(v).into_iter().map(Value::String).collect();
            site.insert(WebSite::HOSTS.key().into(), Value::Array(list));
            site
        }
        _ => Map::new(),
    }
}

fn shortest(site: Map<String, Value>) -> Option<Value> {
    let names = names(&Value::Object(site.clone()));
    if names.is_empty() {
        return None;
    }
    if site.keys().any(|k| k != WebSite::HOSTS.key()) {
        return Some(Value::Object(site));
    }
    Some(match names.as_slice() {
        [one] => Value::String(one.clone()),
        _ => Value::Array(names.into_iter().map(Value::String).collect()),
    })
}

pub fn website(existing: Option<&Value>, names: &[String], tls: Option<Value>) -> Option<Value> {
    let mut site = as_site(existing);
    let list = names.iter().map(|n| Value::String(n.clone())).collect();
    site.insert(WebSite::HOSTS.key().into(), Value::Array(list));
    match tls.filter(|t| !t.is_null()) {
        Some(t) => {
            site.insert(WebSite::TLS.key().into(), t);
        }
        None => {
            site.shift_remove(WebSite::TLS.key());
        }
    }
    shortest(site)
}

pub fn with_field(existing: Option<&Value>, key: &str, v: Option<Value>) -> Option<Value> {
    let mut site = as_site(existing);
    match v.filter(|v| !v.is_null()) {
        Some(v) => {
            site.insert(key.into(), v);
        }
        None => {
            site.shift_remove(key);
        }
    }
    shortest(site)
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
        assert_eq!(website(None, &n(&[]), Some(passthrough())), None);
        assert_eq!(
            website(None, &n(&["a.example.com"]), None),
            Some(json!("a.example.com"))
        );
        assert_eq!(
            website(None, &n(&["a.example.com", "b.example.com"]), None),
            Some(json!(["a.example.com", "b.example.com"]))
        );
        let off = website(None, &n(&["a.example.com"]), Some(json!("off"))).unwrap();
        assert_eq!(tls_of(&off), Some(&json!("off")));
        let files = json!({"cert": "c.pem", "key": "k.pem"});
        let own = website(None, &n(&["a.example.com"]), Some(files.clone())).unwrap();
        assert_eq!(tls_of(&own), Some(&files));
        let site = website(None, &n(&["a.example.com"]), Some(passthrough())).unwrap();
        assert_eq!(
            site,
            json!({"hosts": ["a.example.com"], "tls": "passthrough"})
        );
        assert!(passes_through(&site));
        assert!(!passes_through(&json!("a.example.com")));
        assert_eq!(names(&site), ["a.example.com"]);
    }

    #[test]
    fn an_edit_keeps_the_keys_of_the_site_it_does_not_touch() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let site = json!({"hosts": ["a.example.com"], "tls": "off", "https_redirect": true, "future": {"x": 1}});
        let renamed = website(Some(&site), &n(&["b.example.com"]), Some(json!("off"))).unwrap();
        assert_eq!(
            renamed,
            json!({"hosts": ["b.example.com"], "tls": "off", "https_redirect": true, "future": {"x": 1}})
        );
        let auto = website(Some(&renamed), &n(&["b.example.com"]), None).unwrap();
        assert_eq!(
            auto,
            json!({"hosts": ["b.example.com"], "https_redirect": true, "future": {"x": 1}})
        );
        assert_eq!(website(Some(&site), &n(&[]), None), None);
    }

    #[test]
    fn a_site_field_turns_a_short_form_into_a_site_and_back() {
        let short = json!("a.example.com");
        let long = with_field(Some(&short), "https_redirect", Some(json!(false))).unwrap();
        assert_eq!(
            long,
            json!({"hosts": ["a.example.com"], "https_redirect": false})
        );
        assert_eq!(with_field(Some(&long), "https_redirect", None), Some(short));
        let two = json!(["a.example.com", "b.example.com"]);
        let long = with_field(Some(&two), "https_redirect", Some(json!(true))).unwrap();
        assert_eq!(
            with_field(Some(&long), "https_redirect", Some(json!(null))),
            Some(two)
        );
        assert_eq!(with_field(None, "https_redirect", Some(json!(true))), None);
    }

    #[test]
    fn a_bind_comes_first_and_nothing_means_private() {
        let both = json!({"bind": 7840, "web": "a.example.com"});
        assert_eq!(exposure(&both).as_deref(), Some("7840"));
        assert_eq!(exposure(&json!({"target": 22})), None);
        assert!(!is_public(&json!({"bind": null, "web": []})));
    }
}
