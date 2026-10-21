use burrow_admin_proto::schema::{Editable, Field, Node, Schema, SemType, TextC, UnknownType};
use burrow_config::{ServerConfig, ServiceEntry, VisitorEntry};
use serde_json::json;

pub const SCHEMA_ENV: &str = "BURROW_ADMIN_TEST_SCHEMA";

pub fn schema(s: Schema) -> Schema {
    if !cfg!(feature = "admin-test-hooks") {
        return s;
    }
    match std::env::var(SCHEMA_ENV).as_deref() {
        Ok("future") => future(s),
        Ok("older") => older(s),
        _ => s,
    }
}

const OLDER_SERVICE: [&str; 3] = [
    ServiceEntry::BIND.key(),
    ServiceEntry::HEALTH_CHECK.key(),
    ServiceEntry::MODE.key(),
];
const OLDER_VISITOR: [&str; 1] = [VisitorEntry::ADMIN_FROM.key()];

pub fn older(mut s: Schema) -> Schema {
    if let SemType::Object(root) = &mut s.root.sem {
        for f in &mut root.fields {
            let gone: &[&str] = if f.key == ServerConfig::SERVICES.key() {
                &OLDER_SERVICE
            } else if f.key == ServerConfig::VISITORS.key() {
                &OLDER_VISITOR
            } else {
                continue;
            };
            if let SemType::Map(m) = &mut f.node.sem
                && let SemType::Object(o) = &mut m.value.sem
            {
                o.fields.retain(|x| !gone.contains(&x.key.as_str()));
            }
        }
    }
    s.hash = format!("{}-older", s.hash);
    s
}

pub fn future(mut s: Schema) -> Schema {
    let services = s
        .root
        .field(ServerConfig::SERVICES.key())
        .map(|f| f.node.clone())
        .and_then(|n| match n.sem {
            SemType::Map(m) => Some(*m.value),
            _ => None,
        });
    let Some(mut service) = services else {
        return s;
    };
    if let SemType::Object(o) = &mut service.sem {
        if let Some(f) = o
            .fields
            .iter_mut()
            .find(|f| f.key == ServiceEntry::BANDWIDTH.key())
        {
            f.node.sem = SemType::Unknown(UnknownType {
                t: "rate_v2".into(),
                fallback: vec![SemType::Text(TextC::default())],
                raw: json!({"t": "rate_v2", "fallback": [{"t": "text"}]}),
            });
        }
        o.fields.push(Field {
            key: "region".into(),
            node: Node {
                label: "Region".into(),
                help: "Where the service may be reached from, in a format of a newer burrow".into(),
                editable: Editable::No {
                    reason: "set by a newer server".into(),
                },
                group: "access".into(),
                ..Node::new(SemType::Unknown(UnknownType {
                    t: "geo_area".into(),
                    fallback: Vec::new(),
                    raw: json!({"t": "geo_area"}),
                }))
            },
        });
    }
    if let SemType::Object(root) = &mut s.root.sem
        && let Some(f) = root
            .fields
            .iter_mut()
            .find(|f| f.key == ServerConfig::SERVICES.key())
        && let SemType::Map(m) = &mut f.node.sem
    {
        *m.value = service;
    }
    s.hash = format!("{}-future", s.hash);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_future_schema_has_a_newer_type_and_an_unknown_node() {
        let s = future(crate::config::admin_schema::server_schema());
        let svc = s
            .root
            .at(&["services".to_string(), "x".to_string()])
            .expect("service");
        let bw = &svc.field("bandwidth").expect("bandwidth").node;
        assert_eq!(bw.sem.tag(), "rate_v2");
        let region = &svc.field("region").expect("region").node;
        assert_eq!(region.sem.tag(), "geo_area");
        let text = serde_json::to_string(&s).expect("serializes");
        let back: Schema = serde_json::from_str(&text).expect("an older UI still reads it");
        assert_eq!(back.hash, s.hash);
        assert!(s.hash.ends_with("-future"));
        assert_eq!(
            schema(crate::config::admin_schema::server_schema()).hash,
            crate::config::admin_schema::server_schema().hash
        );
    }
}
