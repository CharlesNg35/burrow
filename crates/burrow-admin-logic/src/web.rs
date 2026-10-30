use burrow_admin_proto::serde_json::{Map, json};
use burrow_config::{ServiceEntry, TlsMode, WebSite};

use crate::Value;

pub const TURN_ON: &str =
    "# server.yaml\nweb:\n  https: 443\n  http: 80\n  letsencrypt:\n    email: you@example.com\n";

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

pub fn is_plain(tls: Option<&Value>, https_on: bool) -> bool {
    match tls.filter(|t| !t.is_null()) {
        Some(t) => t.as_str().and_then(TlsMode::from_wire) == Some(TlsMode::Off),
        None => !https_on,
    }
}

pub fn site_name_problem(text: &str, plain: bool) -> Option<String> {
    use burrow_syntax::web::{SingleLabel, host_name};
    let single = if plain {
        SingleLabel::Allowed
    } else {
        SingleLabel::Refused
    };
    host_name(text, single).err()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ways {
    pub public: bool,
    pub website: bool,
    pub visitors: bool,
}

pub fn way_on(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Array(a)) => !a.is_empty(),
        Some(_) => true,
    }
}

pub fn ways(svc: &Value) -> Ways {
    Ways {
        public: way_on(ServiceEntry::BIND.get(svc)),
        website: way_on(ServiceEntry::WEB.get(svc)),
        visitors: way_on(ServiceEntry::VISITORS.get(svc)),
    }
}

pub const VISITORS_ONLY: &str = "visitors only";
pub const WEBSITE_ONLY: &str = "website only";
pub const NO_WAY_IN: &str = "no way in";

pub fn without_exposure(w: Ways) -> &'static str {
    match w {
        Ways { website: true, .. } => WEBSITE_ONLY,
        Ways { visitors: true, .. } => VISITORS_ONLY,
        _ => NO_WAY_IN,
    }
}

pub fn reached(svc: &Value) -> String {
    exposure(svc).unwrap_or_else(|| without_exposure(ways(svc)).to_string())
}

pub fn is_public(svc: &Value) -> bool {
    exposure(svc).is_some()
}

pub fn service_name(names: &[String], taken: &[String]) -> String {
    let first = names
        .first()
        .and_then(|n| n.trim_start_matches("*.").split('.').next())
        .filter(|l| {
            burrow_syntax::name::check_name(l).is_ok() && !l.chars().all(|c| c.is_ascii_digit())
        })
        .unwrap_or("site");
    let mut name = first.to_string();
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{first}-{n}");
        n += 1;
    }
    name
}

pub fn needs_letsencrypt(hosts: &[String], web: &Value) -> bool {
    if burrow_config::WebConfig::LETSENCRYPT
        .get(web)
        .is_some_and(|v| !v.is_null())
    {
        return false;
    }
    let entries: Vec<Vec<String>> = burrow_config::WebConfig::CERTIFICATES
        .get(web)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(names).collect())
        .unwrap_or_default();
    hosts.iter().any(|n| {
        !matches!(
            burrow_syntax::web::entry_for(entries.iter().map(Vec::as_slice).enumerate(), n),
            Ok(Some(_))
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteKind {
    Https,
    Plain,
    Passthrough,
}

pub fn site_kind(svc: &Value, https_on: bool) -> Option<SiteKind> {
    let web = ServiceEntry::WEB.get(svc).filter(|w| !w.is_null())?;
    if names(web).is_empty() {
        return None;
    }
    Some(
        match tls_of(web)
            .and_then(Value::as_str)
            .and_then(TlsMode::from_wire)
        {
            Some(TlsMode::Passthrough) => SiteKind::Passthrough,
            Some(TlsMode::Off) => SiteKind::Plain,
            None if !https_on => SiteKind::Plain,
            _ => SiteKind::Https,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn a_service_is_reached_by_any_mix_of_three_ways() {
        let all = json!({"bind": 9000, "web": "nas.example.com", "visitors": ["laptop"]});
        assert_eq!(
            ways(&all),
            Ways {
                public: true,
                website: true,
                visitors: true
            }
        );
        let none = json!({"visitors": [], "web": null});
        assert_eq!(ways(&none), Ways::default());
    }

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
    fn the_redirect_choice_applies_only_where_burrow_does_https() {
        use crate::relations::field_state;
        use crate::tree::{fields, inner};
        use burrow_admin_proto::schema::SemType;
        let schema = crate::testing::schema();
        let service = schema.root.at(&["services".into(), "x".into()]).unwrap();
        let web = fields(service).iter().find(|f| f.key == "web").unwrap();
        let SemType::Union(u) = &inner(&web.node).sem else {
            panic!("web is a union")
        };
        let redirect = u
            .branches
            .iter()
            .flat_map(|b| fields(b).iter())
            .find(|f| f.key == WebSite::HTTPS_REDIRECT.key())
            .unwrap();
        let base: Vec<String> = ["services", "nas", "web"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let shown = |site: Value| {
            let doc = json!({"services": {"nas": {"web": site}}});
            field_state(&redirect.node, &schema.root, &doc, &base).enabled
        };
        assert!(shown(json!({"hosts": ["a.example.com"]})));
        assert!(shown(json!({"hosts": ["a.example.com"], "tls": "auto"})));
        assert!(shown(json!("a.example.com")));
        assert!(!shown(
            json!({"hosts": ["a.example.com"], "tls": "passthrough"})
        ));
        assert!(!shown(json!({"hosts": ["a.example.com"], "tls": "off"})));
    }

    #[test]
    fn a_new_website_gets_a_free_service_name_from_its_first_name() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(service_name(&n(&["blog.example.com"]), &n(&[])), "blog");
        assert_eq!(
            service_name(&n(&["blog.example.com"]), &n(&["blog"])),
            "blog-2"
        );
        assert_eq!(service_name(&n(&["*.apps.example.com"]), &n(&[])), "apps");
        assert_eq!(service_name(&n(&["203.0.113.10"]), &n(&[])), "site");
        assert_eq!(service_name(&n(&[]), &n(&["site"])), "site-2");
    }

    #[test]
    fn a_new_website_asks_for_an_email_only_when_no_certificate_covers_it() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let bare = json!({"https": 443});
        assert!(needs_letsencrypt(&n(&["blog.example.com"]), &bare));
        let le = json!({"https": 443, "letsencrypt": {"email": "a@b.c"}});
        assert!(!needs_letsencrypt(&n(&["blog.example.com"]), &le));
        let files = json!({"https": 443, "certificates": [{"hosts": ["*.example.com"], "cert": "c", "key": "k"}]});
        assert!(!needs_letsencrypt(&n(&["blog.example.com"]), &files));
        assert!(needs_letsencrypt(&n(&["example.com"]), &files));
    }

    #[test]
    fn a_website_is_https_plain_or_passed_through() {
        let svc = |web: Value| json!({"target": 80, "web": web});
        assert_eq!(
            site_kind(&svc(json!("a.example.com")), true),
            Some(SiteKind::Https)
        );
        assert_eq!(
            site_kind(&svc(json!("a.example.com")), false),
            Some(SiteKind::Plain)
        );
        assert_eq!(
            site_kind(
                &svc(json!({"hosts": ["a.example.com"], "tls": "off"})),
                true
            ),
            Some(SiteKind::Plain)
        );
        assert_eq!(
            site_kind(
                &svc(json!({"hosts": ["a.example.com"], "tls": "passthrough"})),
                true
            ),
            Some(SiteKind::Passthrough)
        );
        assert_eq!(site_kind(&json!({"target": 80}), true), None);
    }

    #[test]
    fn a_bind_comes_first_and_nothing_means_private() {
        let both = json!({"bind": 7840, "web": "a.example.com"});
        assert_eq!(exposure(&both).as_deref(), Some("7840"));
        assert_eq!(exposure(&json!({"target": 22})), None);
        assert!(!is_public(&json!({"bind": null, "web": []})));
    }

    #[test]
    fn a_service_without_a_public_address_says_how_it_is_reached() {
        use burrow_admin_proto::serde_json::json;
        assert_eq!(reached(&json!({"bind": 9000})), "9000");
        assert_eq!(
            reached(&json!({"web": {"hosts": ["a.example.com"]}})),
            "a.example.com"
        );
        assert_eq!(reached(&json!({"visitors": ["laptop"]})), VISITORS_ONLY);
        assert_eq!(reached(&json!({"web": {"hosts": []}})), WEBSITE_ONLY);
        assert_eq!(reached(&json!({"target": 1})), NO_WAY_IN);
    }
}
