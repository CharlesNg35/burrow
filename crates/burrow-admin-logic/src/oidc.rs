use std::collections::BTreeMap;

use burrow_admin_proto::serde_json::{self, Map, Value};
use burrow_config::{
    Entry, OidcConfig, OidcPick, OidcSetting, ServerConfig, ServiceEntry, WebConfig, WebSite,
};

pub const HIDDEN_SECRET: &str = "written in the file (hidden)";
pub const PUBLIC_CLIENT: &str = "none (public client)";

pub fn providers(doc: &Value) -> Map<String, Value> {
    ServerConfig::WEB
        .get(doc)
        .and_then(|w| WebConfig::OIDC.get(w))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub fn chosen(site: &Value, all: &Map<String, Value>) -> Vec<String> {
    let Some(setting) = WebSite::OIDC
        .get(site)
        .and_then(|v| serde_json::from_value::<OidcSetting>(v.clone()).ok())
    else {
        return Vec::new();
    };
    let names: BTreeMap<String, ()> = all.keys().map(|k| (k.clone(), ())).collect();
    match setting.pick(&names) {
        OidcPick::Found(found) => found.into_iter().map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

pub fn site_value(names: &[String]) -> Option<Value> {
    match names {
        [] => None,
        [one] => Some(Value::String(one.clone())),
        many => Some(Value::Array(
            many.iter().cloned().map(Value::String).collect(),
        )),
    }
}

pub fn used_by(doc: &Value, name: &str) -> Vec<String> {
    let all = providers(doc);
    ServiceEntry::entries(doc)
        .map(|m| {
            m.iter()
                .filter(|(_, svc)| {
                    ServiceEntry::WEB
                        .get(svc)
                        .is_some_and(|w| chosen(w, &all).iter().any(|n| n == name))
                })
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default()
}

pub fn config(v: &Value) -> Option<OidcConfig> {
    serde_json::from_value(v.clone()).ok()
}

pub fn secret_shown(c: &OidcConfig) -> String {
    match c.client_secret.as_deref() {
        None => PUBLIC_CLIENT.to_string(),
        Some(s) if burrow_syntax::env::is_reference(s) => format!("from {}", s.trim()),
        Some(_) => HIDDEN_SECRET.to_string(),
    }
}

pub fn summary(c: &OidcConfig) -> String {
    format!("{}, client ID {}", c.issuer.trim(), c.client_id.trim())
}

pub fn note(names: &[String], all: &Map<String, Value>) -> String {
    let issuers: Vec<String> = names
        .iter()
        .filter_map(|n| {
            let c = all.get(n).and_then(config)?;
            Some(format!("{n} ({})", c.issuer.trim()))
        })
        .collect();
    let chooser = if names.len() > 1 {
        "; visitors pick one on a small page"
    } else {
        ""
    };
    format!(
        "Log in with {}{chooser}; the app gets Remote-User and Remote-Email",
        issuers.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    fn doc() -> Value {
        json!({
            "web": {"oidc": {
                "google": {"issuer": "https://accounts.google.com", "client_id": "123.apps", "client_secret": "${GOOGLE_SECRET}"},
                "work": {"issuer": "https://sso.example.com/realms/home", "client_id": "burrow"}
            }},
            "services": {
                "wiki": {"web": {"hosts": ["w.example.com"], "oidc": "google"}},
                "dav": {"web": {"hosts": ["d.example.com"], "oidc": ["work", "google"]}},
                "blog": {"web": {"hosts": ["b.example.com"], "oidc": true}},
                "api": {"target": 1}
            }
        })
    }

    #[test]
    fn a_provider_is_used_by_the_sites_that_name_it_alone_or_in_a_list() {
        let d = doc();
        assert_eq!(used_by(&d, "google"), ["wiki", "dav"]);
        assert_eq!(used_by(&d, "work"), ["dav"]);
        let one = json!({
            "web": {"oidc": {"g": {"issuer": "https://a.example.com", "client_id": "x"}}},
            "services": {"shop": {"web": {"hosts": ["s.example.com"], "oidc": true}}}
        });
        assert_eq!(used_by(&one, "g"), ["shop"], "true means the only one");
    }

    #[test]
    fn one_name_is_written_as_text_several_as_a_list_and_none_removes_the_key() {
        assert_eq!(site_value(&[]), None);
        assert_eq!(site_value(&["g".into()]), Some(json!("g")));
        assert_eq!(
            site_value(&["g".into(), "w".into()]),
            Some(json!(["g", "w"]))
        );
    }

    #[test]
    fn the_secret_is_shown_by_its_reference_only() {
        let all = providers(&doc());
        let google = config(&all["google"]).unwrap();
        assert_eq!(secret_shown(&google), "from ${GOOGLE_SECRET}");
        assert_eq!(secret_shown(&config(&all["work"]).unwrap()), PUBLIC_CLIENT);
        let inline = OidcConfig {
            client_secret: Some("s3cr3t-value".into()),
            ..google.clone()
        };
        assert_eq!(secret_shown(&inline), HIDDEN_SECRET);
        assert_eq!(
            summary(&google),
            "https://accounts.google.com, client ID 123.apps"
        );
    }

    #[test]
    fn the_site_note_names_the_providers_and_the_headers() {
        let all = providers(&doc());
        assert_eq!(
            note(&["google".into()], &all),
            "Log in with google (https://accounts.google.com); the app gets Remote-User and Remote-Email"
        );
        assert!(note(&["work".into(), "google".into()], &all).contains("visitors pick one"));
    }
}
