use burrow_admin_proto::msg::{self, GeneratedSecret};
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use burrow_syntax::name::check_name;

use crate::api;
use crate::store::store;
use leptos::prelude::*;

pub fn singular(section: &str) -> &'static str {
    burrow_admin_logic::entity::of_section(section).singular
}

pub fn taken(section: &str) -> Vec<String> {
    store().edit.draft.with(|d| {
        d.draft()
            .get(section)
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    })
}

pub fn name_problem(name: &str, section: &str, except: Option<&str>) -> Option<String> {
    if let Err(e) = check_name(name) {
        return Some(e);
    }
    if section == ClientEntry::section()
        && crate::format::reserved_clients().iter().any(|r| r == name)
    {
        return Some(format!(
            "{name} is reserved for this server; give the client another name"
        ));
    }
    (taken(section).iter().any(|t| t == name) && except != Some(name))
        .then(|| format!("a {} named {name} already exists", singular(section)))
}

fn template(section: &str) -> Value {
    let mut v = json!({});
    if section == ServiceEntry::section() {
        let doc = store().edit.doc();
        let reserved = crate::format::reserved_clients();
        if let Some(c) = burrow_admin_logic::choices::default_client(&doc, &reserved) {
            ServiceEntry::CLIENT.set(&mut v, Value::String(c));
        }
        ServiceEntry::TARGET.set(&mut v, Value::from(8080));
    }
    v
}

pub fn create_with(
    section: &str,
    name: &str,
    secret: Option<(String, String)>,
    max: Option<u64>,
    links: &[String],
) {
    let e = store().edit;
    let path = vec![section.to_string(), name.to_string()];
    let mut value = template(section);
    if let Some(m) = max
        && value.is_object()
    {
        if section == ClientEntry::section() {
            ClientEntry::MAX_CONNECTIONS.set(&mut value, Value::from(m));
        } else {
            VisitorEntry::MAX_CONNECTIONS.set(&mut value, Value::from(m));
        }
    }
    e.set(&path, Some(value));
    let doc = e.doc();
    for svc in links {
        let Some(v) = ServiceEntry::entry(&doc, svc) else {
            continue;
        };
        if section == ClientEntry::section() {
            let mut entries = ServiceEntry::with_client(v, name);
            let client = ServiceEntry::CLIENT.of(svc);
            let clients = ServiceEntry::CLIENTS.of(svc);
            if entries.len() == 1 {
                e.set(&client, Some(entries.remove(0)));
            } else {
                e.set(&client, None);
                e.set(&clients, Some(Value::Array(entries)));
            }
        } else {
            let visitors = ServiceEntry::VISITORS.of(svc);
            let mut list: Vec<Value> = ServiceEntry::VISITORS
                .get(v)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            list.push(Value::String(name.to_string()));
            e.set(&visitors, Some(Value::Array(list)));
        }
    }
    if section != ServiceEntry::section() {
        let mut secret_path = vec![section.to_string(), name.to_string()];
        secret_path.extend(if section == ClientEntry::section() {
            ClientEntry::SECRET.rel()
        } else {
            VisitorEntry::SECRET.rel()
        });
        match secret {
            Some((handle, fp)) => e.add_secret(&secret_path, &handle, &fp),
            None => api::request(msg::req::GENERATE_SECRET, json!({}), move |r| {
                if let Ok(Ok(g)) = r.map(serde_json::from_value::<GeneratedSecret>) {
                    e.add_secret(&secret_path, &g.handle, &g.fp);
                }
            }),
        }
    }
}
