use burrow_admin_proto::Path;
use burrow_admin_proto::serde_json::Value;

use crate::field::Entry;
use crate::model::{ClientEntry, ServerConfig, ServiceEntry, VisitorEntry};

pub fn rename_refs(root: &mut Value, section: &str, from: &str, to: &str) -> Vec<Path> {
    let mut keys = Vec::new();
    let fields = if section == ClientEntry::section() {
        vec![ServiceEntry::CLIENT.key(), ServiceEntry::CLIENTS.key()]
    } else if section == VisitorEntry::section() {
        vec![ServiceEntry::VISITORS.key()]
    } else {
        Vec::new()
    };
    if let Some(services) = ServerConfig::SERVICES
        .get_mut(root)
        .and_then(Value::as_object_mut)
    {
        for svc in services.values_mut() {
            let Some(svc) = svc.as_object_mut() else {
                continue;
            };
            for f in &fields {
                match svc.get_mut(*f) {
                    Some(Value::Array(items)) => items.iter_mut().for_each(|v| {
                        if v.as_str() == Some(from) {
                            *v = Value::String(to.to_string());
                        }
                    }),
                    Some(v) if v.as_str() == Some(from) => *v = Value::String(to.to_string()),
                    _ => {}
                }
            }
        }
    }
    if section == ClientEntry::section()
        && let Some(visitors) = ServerConfig::VISITORS
            .get_mut(root)
            .and_then(Value::as_object_mut)
    {
        for (name, v) in visitors.iter_mut() {
            if let Some(reach) = VisitorEntry::REACH
                .get_mut(v)
                .and_then(Value::as_object_mut)
                && reach.contains_key(from)
            {
                keys.push(VisitorEntry::REACH.entry_of(name, from));
                let renamed: Vec<(String, Value)> = std::mem::take(reach)
                    .into_iter()
                    .map(|(k, x)| (if k == from { to.to_string() } else { k }, x))
                    .collect();
                reach.extend(renamed);
            }
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn a_renamed_client_keeps_its_place_in_every_reference() {
        let mut doc = json!({
            "services": {
                "web": {"client": "home", "target": 80},
                "db": {"clients": ["office", "home"], "target": 5432},
                "ssh": {"visitors": ["home"], "target": 22}
            },
            "visitors": {"lap": {"reach": {"home": "any", "office": "any", "lab": "any"}}}
        });
        let keys = rename_refs(&mut doc, ClientEntry::section(), "home", "home2");
        assert_eq!(keys, [VisitorEntry::REACH.entry_of("lap", "home")]);
        assert_eq!(doc["services"]["web"]["client"], "home2");
        assert_eq!(doc["services"]["db"]["clients"], json!(["office", "home2"]));
        assert_eq!(doc["services"]["ssh"]["visitors"], json!(["home"]));
        let keys: Vec<&String> = doc["visitors"]["lap"]["reach"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(keys, ["home2", "office", "lab"]);
        rename_refs(&mut doc, VisitorEntry::section(), "home", "guest");
        assert_eq!(doc["services"]["ssh"]["visitors"], json!(["guest"]));
        assert_eq!(doc["services"]["web"]["client"], "home2");
    }
}
