use leptos::prelude::*;

use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_config::{ServerConfig, TlsMode, WebConfig};
use burrow_syntax::web::CheckPorts;

use crate::store::store;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Ports {
    pub(super) on: bool,
    pub(super) https: bool,
    pub(super) http: bool,
    pub(super) numbers: CheckPorts,
}

fn shown_port(v: Option<&Value>) -> String {
    match v {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn port_of(v: &Value) -> Option<u16> {
    match v {
        Value::Number(n) => n.as_u64().and_then(|p| u16::try_from(p).ok()),
        Value::String(s) => s.rsplit(':').next().and_then(|p| p.trim().parse().ok()),
        _ => None,
    }
}

pub(super) fn web_port(plain: bool) -> String {
    store().edit.draft.with(|d| {
        let web = ServerConfig::WEB.get(d.draft());
        let (scheme, field) = if plain {
            ("HTTP", web.and_then(|w| WebConfig::HTTP.get(w)))
        } else {
            ("HTTPS", web.and_then(|w| WebConfig::HTTPS.get(w)))
        };
        let port = shown_port(field);
        if port.is_empty() {
            String::new()
        } else {
            format!("{scheme} {port}")
        }
    })
}

pub(super) fn web_now() -> Option<Value> {
    store().edit.draft.with(|d| {
        ServerConfig::WEB
            .get(d.draft())
            .filter(|w| !w.is_null())
            .cloned()
    })
}

fn port_value(text: &str) -> Option<Value> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    Some(t.parse::<u16>().map_or_else(|_| json!(t), |n| json!(n)))
}

pub(super) fn port_problem(label: &str, text: &str) -> Option<String> {
    let v = port_value(text)?;
    let ok = serde_json::from_value::<burrow_config::Bind>(v)
        .ok()
        .and_then(|b| b.ranged([0, 0, 0, 0], label).ok())
        .is_some_and(|(_, n)| n == 1);
    (!ok).then(|| format!("{label}: give one port, like 443, or an address like 0.0.0.0:443"))
}

pub(super) fn planned(https: &str, http: &str) -> Value {
    let mut web = serde_json::Map::new();
    if let Some(v) = port_value(https) {
        web.insert(WebConfig::HTTPS.key().into(), v);
    }
    if let Some(v) = port_value(http) {
        web.insert(WebConfig::HTTP.key().into(), v);
    }
    Value::Object(web)
}

pub(super) fn ports_of(web: Option<&Value>) -> Ports {
    let has = |w: Option<&Value>, f: fn(&Value) -> Option<&Value>| {
        w.and_then(f).is_some_and(|v| !v.is_null())
    };
    Ports {
        on: web.is_some(),
        https: has(web, |w| WebConfig::HTTPS.get(w)),
        http: has(web, |w| WebConfig::HTTP.get(w)),
        numbers: CheckPorts {
            https: web.and_then(|w| WebConfig::HTTPS.get(w)).and_then(port_of),
            http: web.and_then(|w| WebConfig::HTTP.get(w)).and_then(port_of),
        },
    }
}

pub(super) fn tls_options(p: Ports) -> Vec<burrow_ui_kit::RadioOption> {
    crate::domain::website::https_options(|v| {
        let blocked = if v == TlsMode::Off.wire() {
            !p.http
        } else {
            !p.https
        };
        blocked.then(|| {
            if v == TlsMode::Off.wire() {
                "needs the web proxy's HTTP port".to_string()
            } else {
                "needs the web proxy's HTTPS port".to_string()
            }
        })
    })
}

pub(super) fn https_problem(names: &[String], tls: &str, web: &Value, p: Ports) -> Option<String> {
    match TlsMode::from_wire(tls) {
        Some(TlsMode::Auto) if p.https => {
            burrow_admin_logic::web::letsencrypt_problem(names, web, p.numbers)
        }
        Some(TlsMode::Internal) => names
            .iter()
            .find_map(|n| burrow_admin_logic::cert_words::local_ca(n)),
        _ => None,
    }
}
