use burrow_admin_proto::schema::{Node, SemType};
use burrow_admin_proto::value::{self, EnvInfo, SecretInfo, SecretSrc};
use serde_json::{Map, Value};

use crate::config::check::secretish;
use crate::config::env::{EnvSource, expand_value};
use crate::wire::secret::Secret;

fn holds_names(node: Option<&Node>) -> bool {
    match node.map(|n| &n.sem) {
        Some(SemType::Map(_) | SemType::RefMap(_)) => true,
        Some(SemType::Optional(o)) => holds_names(Some(&o.inner)),
        _ => false,
    }
}

fn child<'a>(node: Option<&'a Node>, key: &str) -> Option<&'a Node> {
    let node = node?;
    match &node.sem {
        SemType::Object(o) => o.fields.iter().find(|f| f.key == key).map(|f| &f.node),
        SemType::Map(m) => Some(&m.value),
        SemType::RefMap(m) => Some(&m.value),
        SemType::Optional(o) => child(Some(&o.inner), key),
        _ => None,
    }
}

fn is_secret(node: Option<&Node>) -> bool {
    match node.map(|n| &n.sem) {
        Some(SemType::SecretRef(_)) => true,
        Some(SemType::Optional(o)) => is_secret(Some(&o.inner)),
        _ => false,
    }
}

fn is_reference(node: Option<&Node>) -> bool {
    match node.map(|n| &n.sem) {
        Some(SemType::SecretRef(c)) => c.reference,
        Some(SemType::Optional(o)) => is_reference(Some(&o.inner)),
        _ => false,
    }
}

fn is_path(node: Option<&Node>) -> bool {
    match node.map(|n| &n.sem) {
        Some(SemType::Path(_)) => true,
        Some(SemType::Optional(o)) => is_path(Some(&o.inner)),
        _ => false,
    }
}

fn secret_info(raw: &str, env: &dyn EnvSource) -> SecretInfo {
    let vars = burrow_syntax::env::names(raw);
    let src = match vars.first() {
        Some(v) if raw.contains("${") => SecretSrc::Env(v.clone()),
        _ => SecretSrc::Literal,
    };
    let expanded = if raw.contains("${") {
        expand_value(raw, env)
    } else {
        Some(raw.to_string())
    };
    let unset = expanded.is_none();
    SecretInfo {
        fp: expanded.and_then(|t| Secret::parse(&t).ok().map(|s| s.fingerprint())),
        src,
        unset,
    }
}

pub fn fingerprint_of(raw: &str, env: &dyn EnvSource) -> Option<String> {
    secret_info(raw, env).fp
}

fn present_secret(v: &Value, env: &dyn EnvSource) -> Value {
    match v {
        Value::String(s) => value::secret(&secret_info(s, env)),
        Value::Array(items) => Value::Array(items.iter().map(|i| present_secret(i, env)).collect()),
        Value::Null => Value::Null,
        other => value::secret(&SecretInfo {
            fp: None,
            src: SecretSrc::Literal,
            unset: false,
        })
        .as_object()
        .map_or_else(|| other.clone(), |_| value::secret(&secret_info("", env))),
    }
}

pub fn present(v: &Value, node: Option<&Node>, key: &str, env: &dyn EnvSource) -> Value {
    present_at(v, node, key, false, env)
}

fn present_at(
    v: &Value,
    node: Option<&Node>,
    key: &str,
    is_name: bool,
    env: &dyn EnvSource,
) -> Value {
    if is_reference(node)
        && let Value::String(s) = v
        && burrow_syntax::env::is_reference(s)
    {
        return v.clone();
    }
    if is_secret(node) || (!is_name && secretish(key) && !is_path(node)) {
        return present_secret(v, env);
    }
    match v {
        Value::Object(m) => {
            let names = holds_names(node);
            let mut out = Map::new();
            for (k, val) in m {
                out.insert(k.clone(), present_at(val, child(node, k), k, names, env));
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            let item = match node.map(|n| &n.sem) {
                Some(SemType::List(l)) => Some(&*l.item),
                _ => None,
            };
            Value::Array(items.iter().map(|i| present(i, item, key, env)).collect())
        }
        Value::String(s) if s.contains("${") => {
            let resolved = expand_value(s, env);
            if resolved.as_deref().is_some_and(super::text::holds_secret) {
                return value::secret(&secret_info(s, env));
            }
            value::env(&EnvInfo {
                raw: s.clone(),
                vars: burrow_syntax::env::names(s),
                resolved,
            })
        }
        Value::String(s) if super::text::holds_secret(s) => value::secret(&secret_info(s, env)),
        other => other.clone(),
    }
}

pub fn raw_of(v: &Value) -> Value {
    if let Some(e) = value::as_env(v) {
        return Value::String(e.raw);
    }
    match v {
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), raw_of(v))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(raw_of).collect()),
        other => other.clone(),
    }
}

pub fn has_secret_marker(v: &Value) -> bool {
    if value::as_secret(v).is_some() {
        return true;
    }
    match v {
        Value::Object(m) => m.values().any(has_secret_marker),
        Value::Array(a) => a.iter().any(has_secret_marker),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_dns_secret_travels_as_its_reference_and_an_inline_one_stays_masked() {
        let schema = crate::config::admin_schema::server_schema();
        let env = |k: &str| (k == "CF_TOKEN").then(|| "cf-secret-value".to_string());
        let raw = json!({"web": {"letsencrypt": {"email": "a@b.c", "dns": [
            {"domains": ["example.com"], "cloudflare": {"token": "${CF_TOKEN}"}},
            {"domains": ["example.net"], "cloudflare": {"token": "pasted-inline"}},
            {"domains": ["example.org"], "rfc2136": {"server": "ns:53", "key_name": "k", "secret": "file:tsig.secret"}}
        ]}}});
        let out = present(&raw, Some(&schema.root), "", &env);
        let dns = &out["web"]["letsencrypt"]["dns"];
        assert_eq!(dns[0]["cloudflare"]["token"], "${CF_TOKEN}");
        assert_eq!(dns[2]["rfc2136"]["secret"], "file:tsig.secret");
        let text = out.to_string();
        assert!(
            !text.contains("pasted-inline") && !text.contains("cf-secret-value"),
            "{text}"
        );
    }

    #[test]
    fn secrets_and_variables_never_travel_as_text() {
        let schema = crate::config::admin_schema::server_schema();
        let s = Secret::generate();
        let env = |k: &str| {
            (k == "HOME")
                .then(|| s.encode())
                .or((k == "PORT").then(|| "7840".to_string()))
        };
        let raw = json!({
            "listen": "${PORT}",
            "clients": {"home": {"secret": "${HOME}"}, "office": {"secret": [s.encode(), "${GONE}"]}},
            "services": {"web": {"client": "home", "target": 3000}}
        });
        let out = present(&raw, Some(&schema.root), "", &env);
        assert_eq!(out["listen"]["$env"]["resolved"], "7840");
        assert_eq!(
            out["clients"]["home"]["secret"]["$secret"]["fp"],
            s.fingerprint()
        );
        assert_eq!(
            out["clients"]["home"]["secret"]["$secret"]["src"]["env"],
            "HOME"
        );
        assert_eq!(
            out["clients"]["office"]["secret"][0]["$secret"]["src"],
            "literal"
        );
        assert_eq!(
            out["clients"]["office"]["secret"][1]["$secret"]["unset"],
            true
        );
        assert!(!out.to_string().contains(&s.encode()));
        assert_eq!(raw_of(&out["listen"]), json!("${PORT}"));
        assert_eq!(out["services"]["web"]["target"], 3000);
    }

    #[test]
    fn secrets_in_the_wrong_shape_or_under_a_wrong_key_stay_masked() {
        let schema = crate::config::admin_schema::server_schema();
        let s = Secret::generate();
        let env = |k: &str| (k == "HOME_SECRET").then(|| s.encode());
        for raw in [
            json!({"clients": {"home": s.encode()}}),
            json!({"clients": {"home": {"secert": s.encode()}}}),
            json!({"clients": {"home": "${HOME_SECRET}"}}),
            json!({"clients": {"home": {"note": "${HOME_SECRET}"}}}),
            json!({"server": format!("wss://u:{}@x.example", s.encode())}),
            json!({"limits": {"other": [s.encode()]}}),
        ] {
            let out = present(&raw, Some(&schema.root), "", &env);
            let text = out.to_string();
            assert!(!text.contains(&s.encode()), "{raw} gave {text}");
            assert!(has_secret_marker(&out), "{text}");
        }
        let out = present(
            &json!({"clients": {"home": "${HOME_SECRET}"}}),
            Some(&schema.root),
            "",
            &env,
        );
        let marker = &out["clients"]["home"]["$secret"];
        assert_eq!(marker["src"]["env"], "HOME_SECRET");
        assert_eq!(marker["fp"], s.fingerprint());
    }

    #[test]
    fn entries_with_names_close_to_secret_are_shown_as_written() {
        let schema = crate::config::admin_schema::server_schema();
        let env = |_: &str| None;
        let raw = json!({
            "clients": {
                "socket": {"secret": Secret::generate().encode()},
                "secure": {"secret": Secret::generate().encode(), "max_connections": 5},
                "sec": {"secret": Secret::generate().encode()}
            },
            "visitors": {
                "sacred": {"secret": Secret::generate().encode(), "admin": "read"},
                "token": {"secret": Secret::generate().encode(), "admin": "read"}
            },
            "services": {
                "socket": {"client": "secure", "target": 3000},
                "sacred": {"client": "sec", "target": 3001},
                "key": {"client": "sec", "target": 3002}
            }
        });
        let out = present(&raw, Some(&schema.root), "", &env);
        assert_eq!(out["clients"]["secure"]["max_connections"], 5, "{out}");
        assert!(
            value::as_secret(&out["clients"]["socket"]).is_none(),
            "{out}"
        );
        assert!(
            value::as_secret(&out["clients"]["socket"]["secret"]).is_some(),
            "{out}"
        );
        assert_eq!(out["visitors"]["sacred"]["admin"], "read", "{out}");
        assert_eq!(out["visitors"]["token"]["admin"], "read", "{out}");
        assert_eq!(out["services"], raw["services"], "{out}");
        assert_eq!(raw_of(&out["services"]), raw["services"]);
    }

    #[test]
    fn a_tls_key_file_path_is_shown_as_written() {
        let schema = crate::config::admin_schema::server_schema();
        let env = |_: &str| None;
        let raw =
            json!({"websocket": {"listen": 7836, "tls": {"cert": "ws.crt", "key": "ws.key"}}});
        let out = present(&raw, Some(&schema.root), "", &env);
        assert_eq!(out["websocket"]["tls"]["key"], "ws.key", "{out}");
    }
}
