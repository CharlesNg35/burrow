use burrow_admin_proto::schema::{Node, OneOf, SemType};
use burrow_admin_proto::serde_json::{Map, Value};

use crate::tree::{fields, inner};

pub fn groups(node: &Node) -> &[OneOf] {
    match &inner(node).sem {
        SemType::Object(o) => &o.one_of,
        _ => &[],
    }
}

pub fn group_of<'a>(node: &'a Node, key: &str) -> Option<&'a OneOf> {
    groups(node).iter().find(|g| g.holds(key))
}

pub fn leads<'a>(node: &'a Node, key: &str) -> Option<&'a OneOf> {
    groups(node).iter().find(|g| g.leads(key))
}

pub fn follows(node: &Node, key: &str) -> bool {
    group_of(node, key).is_some_and(|g| !g.leads(key))
}

fn is_set(obj: &Value, key: &str) -> bool {
    obj.get(key).is_some_and(|v| !v.is_null())
}

pub fn picked(g: &OneOf, obj: Option<&Value>) -> Option<usize> {
    g.picked(obj?)
}

pub fn filled(g: &OneOf, obj: Option<&Value>, choice: usize) -> bool {
    let (Some(obj), Some(c)) = (obj, g.choices.get(choice)) else {
        return false;
    };
    c.iter().any(|k| {
        obj.get(k).is_some_and(|v| match v {
            Value::Null => false,
            Value::Object(m) => m.values().any(|x| !x.is_null()),
            _ => true,
        })
    })
}

pub fn label(node: &Node, choice: &[String]) -> String {
    let first = choice.first().map(String::as_str).unwrap_or_default();
    fields(node)
        .iter()
        .find(|f| f.key == first)
        .map(|f| f.node.label.clone())
        .unwrap_or_else(|| first.to_string())
}

pub fn options(node: &Node, g: &OneOf) -> Vec<(usize, String)> {
    g.choices
        .iter()
        .enumerate()
        .map(|(i, c)| (i, label(node, c)))
        .collect()
}

pub fn switch(node: &Node, g: &OneOf, obj: Option<&Value>, to: Option<usize>) -> Value {
    let mut out = match obj {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    for (i, c) in g.choices.iter().enumerate() {
        if Some(i) != to {
            for k in c {
                out.remove(k);
            }
        }
    }
    let first = to.and_then(|i| g.choices.get(i)).and_then(|c| c.first());
    if let Some(k) = first
        && !is_set(&Value::Object(out.clone()), k)
        && fields(node)
            .iter()
            .any(|f| &f.key == k && matches!(inner(&f.node).sem, SemType::Object(_)))
    {
        out.insert(k.clone(), Value::Object(Map::new()));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;
    use burrow_config::admin_schema::AdminSchema;
    use burrow_config::{CertFile, DnsEntry};

    #[test]
    fn a_dns_entry_offers_its_providers_as_one_choice() {
        let node = DnsEntry::node();
        let g = leads(&node, DnsEntry::CLOUDFLARE.key()).expect("the provider choice");
        assert!(g.required);
        assert!(follows(&node, DnsEntry::RFC2136.key()));
        assert!(!follows(&node, DnsEntry::DOMAINS.key()));
        let labels: Vec<String> = options(&node, g).into_iter().map(|(_, l)| l).collect();
        assert_eq!(
            labels,
            [
                "Cloudflare",
                "RFC 2136 (BIND, Knot, PowerDNS)",
                "acme-dns",
                "A command of yours"
            ]
        );
    }

    #[test]
    fn the_picked_provider_is_the_one_whose_fields_are_set() {
        let node = DnsEntry::node();
        let g = leads(&node, "cloudflare").unwrap();
        assert_eq!(picked(g, None), None);
        assert_eq!(picked(g, Some(&json!({"domains": ["a.example"]}))), None);
        let v = json!({"domains": ["a.example"], "rfc2136": {"server": "ns:53"}});
        assert_eq!(picked(g, Some(&v)), Some(1));
        assert!(filled(g, Some(&v), 1));
        assert!(!filled(g, Some(&json!({"cloudflare": {}})), 0));
        assert_eq!(picked(g, Some(&json!({"cloudflare": {}}))), Some(0));
    }

    #[test]
    fn switching_the_provider_drops_the_other_ones_and_keeps_the_rest() {
        let node = DnsEntry::node();
        let g = leads(&node, "cloudflare").unwrap();
        let v = json!({"domains": ["example.com"], "cloudflare": {"token": "${CF_TOKEN}"}, "wildcards": ["*.example.com"]});
        assert_eq!(
            switch(&node, g, Some(&v), Some(2)),
            json!({"domains": ["example.com"], "acme_dns": {}, "wildcards": ["*.example.com"]})
        );
        assert_eq!(
            switch(&node, g, Some(&v), Some(3)),
            json!({"domains": ["example.com"], "wildcards": ["*.example.com"]})
        );
        assert_eq!(switch(&node, g, Some(&v), Some(0)), v);
        assert_eq!(
            switch(&node, g, Some(&v), None),
            json!({"domains": ["example.com"], "wildcards": ["*.example.com"]})
        );
    }

    #[test]
    fn a_certificate_takes_its_files_together_or_an_upload() {
        let node = CertFile::node();
        let g = leads(&node, "cert").expect("the source choice");
        assert_eq!(g.choices, [vec!["cert", "key"], vec!["upload"]]);
        assert!(follows(&node, "key") && follows(&node, "upload"));
        let files = json!({"hosts": ["a.example"], "cert": "c.pem", "key": "k.pem"});
        assert_eq!(picked(g, Some(&files)), Some(0));
        assert_eq!(
            switch(&node, g, Some(&files), Some(1)),
            json!({"hosts": ["a.example"]})
        );
    }

    #[test]
    fn every_one_of_in_the_schema_names_fields_of_its_object() {
        fn walk(n: &Node, seen: &mut usize) {
            for g in groups(n) {
                *seen += 1;
                assert!(g.choices.len() >= 2, "{}", g.key);
                for k in g.choices.iter().flatten() {
                    assert!(fields(n).iter().any(|f| &f.key == k), "{}: {k}", g.key);
                }
                assert!(!g.label.is_empty(), "{}", g.key);
            }
            match &n.sem {
                SemType::Object(o) => o.fields.iter().for_each(|f| walk(&f.node, seen)),
                SemType::Optional(o) => walk(&o.inner, seen),
                SemType::List(l) => walk(&l.item, seen),
                SemType::Map(m) => walk(&m.value, seen),
                SemType::RefMap(m) => walk(&m.value, seen),
                SemType::Union(u) => u.branches.iter().for_each(|b| walk(b, seen)),
                _ => {}
            }
        }
        let mut seen = 0;
        walk(&burrow_config::ServerConfig::node(), &mut seen);
        assert!(seen >= 6, "{seen}");
    }
}
