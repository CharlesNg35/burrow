use super::*;
use burrow_admin_proto::schema::{Relation, SemType};
use std::collections::BTreeSet;

fn admin_paths(node: &Node, prefix: &str, out: &mut BTreeSet<String>) {
    match &node.sem {
        SemType::Object(o) => {
            for f in &o.fields {
                let p = format!("{prefix}/{}", f.key);
                out.insert(p.clone());
                admin_paths(&f.node, &p, out);
            }
        }
        SemType::Map(m) => admin_paths(&m.value, &format!("{prefix}/*"), out),
        SemType::RefMap(m) => admin_paths(&m.value, &format!("{prefix}/*"), out),
        SemType::Optional(o) => admin_paths(&o.inner, prefix, out),
        SemType::Union(u) => u.branches.iter().for_each(|b| admin_paths(b, prefix, out)),
        _ => {}
    }
}

fn json_paths(
    v: &serde_json::Value,
    defs: &serde_json::Value,
    prefix: &str,
    out: &mut BTreeSet<String>,
    depth: usize,
) {
    if depth > 12 {
        return;
    }
    if let Some(r) = v.get("$ref").and_then(|r| r.as_str()) {
        let name = r.rsplit('/').next().unwrap_or_default();
        if let Some(d) = defs.get(name) {
            json_paths(d, defs, prefix, out, depth + 1);
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(items) = v.get(key).and_then(|a| a.as_array()) {
            for i in items {
                json_paths(i, defs, prefix, out, depth + 1);
            }
        }
    }
    if let Some(props) = v.get("properties").and_then(|p| p.as_object()) {
        for (k, sub) in props {
            let p = format!("{prefix}/{k}");
            out.insert(p.clone());
            json_paths(sub, defs, &p, out, depth + 1);
        }
    }
    if let Some(ap) = v.get("additionalProperties").filter(|a| a.is_object()) {
        json_paths(ap, defs, &format!("{prefix}/*"), out, depth + 1);
    }
}

#[test]
fn schema_covers_json_schema() {
    let schema = server_schema();
    let mut admin = BTreeSet::new();
    admin_paths(&schema.root, "", &mut admin);
    let json = crate::config::load::schema_json(crate::config::Role::Server);
    let defs = json.get("$defs").cloned().unwrap_or_default();
    let mut from_json = BTreeSet::new();
    json_paths(&json, &defs, "", &mut from_json, 0);
    let missing: Vec<_> = from_json.difference(&admin).collect();
    let extra: Vec<_> = admin.difference(&from_json).collect();
    assert!(missing.is_empty(), "not in the admin schema: {missing:?}");
    assert!(extra.is_empty(), "not in the JSON schema: {extra:?}");
    assert!(admin.contains("/services/*/bind"));
    assert!(admin.contains("/visitors/*/admin_from"));
}

pub(crate) fn walk<'a>(
    node: &'a Node,
    path: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, &'a Node)>,
) {
    out.push((path.clone(), node));
    children(node, path, out);
}

fn children<'a>(node: &'a Node, path: &mut Vec<String>, out: &mut Vec<(Vec<String>, &'a Node)>) {
    match &node.sem {
        SemType::Object(o) => {
            for f in &o.fields {
                path.push(f.key.clone());
                walk(&f.node, path, out);
                path.pop();
            }
        }
        SemType::Map(m) => {
            path.push("*".into());
            walk(&m.value, path, out);
            path.pop();
        }
        SemType::Optional(o) => children(&o.inner, path, out),
        _ => {}
    }
}

fn base(service: &str) -> String {
    let s = || crate::secret::Secret::generate().encode();
    format!(
        "listen: 127.0.0.1:7835\nclients:\n  home:\n    secret: {}\n  office:\n    secret: {}\nvisitors:\n  laptop:\n    secret: {}\n    reach: {{ home: any }}\nservices:\n  web:\n{service}",
        s(),
        s(),
        s()
    )
}

fn collect_paths(
    n: &crate::config::tree::Node,
    path: &mut Vec<String>,
    out: &mut Vec<Vec<String>>,
) {
    use crate::config::tree::Value;
    match &n.value {
        Value::Map(entries) => {
            for e in entries {
                path.push(e.key.clone());
                out.push(path.clone());
                collect_paths(&e.value, path, out);
                path.pop();
            }
        }
        Value::Seq(items) => {
            for (i, it) in items.iter().enumerate() {
                path.push(i.to_string());
                out.push(path.clone());
                collect_paths(it, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

fn diag_paths(yaml: &str) -> Vec<(String, Vec<String>)> {
    use crate::config::check::Spans;
    let nothing = |_: &str| None;
    let outcome = crate::config::load_str(
        "server.yaml",
        yaml,
        crate::config::Format::Yaml,
        Some(crate::config::Role::Server),
        &nothing,
    );
    let root = crate::config::tree::parse(yaml).expect("parses");
    let spans = Spans::new(&root);
    let mut all = Vec::new();
    collect_paths(&root, &mut Vec::new(), &mut all);
    outcome
        .report
        .diagnostics
        .iter()
        .filter_map(|d| {
            let at = d.span?.start.offset;
            let best = all
                .iter()
                .filter(|p| {
                    let refs: Vec<&str> = p.iter().map(String::as_str).collect();
                    let (k, v) = (spans.key(&refs), spans.value(&refs));
                    (k.start.offset <= at && at <= k.end.offset)
                        || (v.start.offset <= at && at < v.end.offset.max(v.start.offset + 1))
                })
                .max_by_key(|p| p.len())?;
            Some((d.code.to_string(), best.clone()))
        })
        .collect()
}

fn reported_on(yaml: &str, path: &[&str]) -> bool {
    diag_paths(yaml)
        .iter()
        .any(|(_, p)| p.len() >= path.len() && p.iter().zip(path).all(|(a, b)| a == b))
}

#[test]
fn relations_agree_with_validation() {
    let schema = server_schema();
    let svc = schema
        .root
        .at(&["services".to_string(), "*".to_string()])
        .expect("service node");
    let SemType::Object(o) = &svc.sem else {
        panic!("services hold objects")
    };
    let sample = |key: &str| match key {
        "mode" => "    mode: balance\n    clients: [home, office]\n",
        "health_check" => "    health_check: true\n    client: home\n",
        "max_connections" => "    max_connections: 5\n    client: home\n",
        "visitors" => "    visitors: [laptop]\n    client: home\n",
        "proxy_protocol" => "    proxy_protocol: v1\n    client: home\n",
        other => panic!("no sample for tcp-only field {other}"),
    };
    let mut checked = 0;
    for f in &o.fields {
        for r in &f.node.relations {
            let (yaml, path) = match r {
                Relation::EnabledIf { .. } => (
                    base(&format!(
                        "    protocol: udp\n    bind: 7840\n    target: 3000\n{}",
                        sample(&f.key)
                    )),
                    vec!["services", "web", f.key.as_str()],
                ),
                Relation::OneOf { .. } => (
                    base(
                        "    client: home\n    clients: [home, office]\n    bind: 7840\n    target: 3000\n",
                    ),
                    vec!["services", "web"],
                ),
                Relation::SameLen(_) => (
                    base("    client: home\n    bind: 30000-30002\n    target: 3000-3001\n"),
                    vec!["services", "web"],
                ),
                _ => continue,
            };
            assert!(
                reported_on(&yaml, &path),
                "{} ({r:?}) is not reported on {path:?}: {:?}",
                f.key,
                diag_paths(&yaml)
            );
            checked += 1;
        }
    }
    assert!(checked >= 7, "only {checked} relations checked");
}

#[test]
fn refs_agree_with_check_refs() {
    let cases: [(&str, &[&str]); 3] = [
        (
            "    client: nobody\n    bind: 7840\n    target: 3000\n",
            &["services", "web", "client"],
        ),
        (
            "    clients: [home, nobody]\n    bind: 7840\n    target: 3000\n",
            &["services", "web", "clients"],
        ),
        (
            "    client: home\n    target: 3000\n    visitors: [nobody]\n",
            &["services", "web", "visitors"],
        ),
    ];
    for (svc, path) in cases {
        let yaml = base(svc);
        assert!(
            reported_on(&yaml, path),
            "{path:?}: {:?}",
            diag_paths(&yaml)
        );
    }
    let yaml = base("    client: home\n    bind: 7840\n    target: 3000\n")
        .replace("reach: { home: any }", "reach: { nobody: any }");
    assert!(
        reported_on(&yaml, &["visitors", "laptop", "reach"]),
        "{:?}",
        diag_paths(&yaml)
    );
    let schema = server_schema();
    let mut nodes = Vec::new();
    walk(&schema.root, &mut Vec::new(), &mut nodes);
    let refs: Vec<String> = nodes
        .iter()
        .filter(|(_, n)| {
            let sem = match &n.sem {
                SemType::Optional(o) => &o.inner.sem,
                s => s,
            };
            matches!(
                sem,
                SemType::Ref(_) | SemType::RefList(_) | SemType::RefMap(_)
            )
        })
        .map(|(p, _)| p.join("."))
        .collect();
    assert_eq!(
        refs,
        [
            "visitors.*.reach",
            "services.*.client",
            "services.*.clients",
            "services.*.visitors"
        ]
    );
}

#[test]
fn admin_rights_are_checked() {
    let svc = "    client: home\n    bind: 7840\n    target: 3000\n";
    let shared = base(svc).replace(
        "    reach: { home: any }\n",
        "    reach: { home: any }\n    admin: write\n",
    );
    let found = diag_paths(&shared);
    assert!(
        found
            .iter()
            .any(|(c, p)| c == "admin-shared" && p.last().is_some_and(|k| k == "admin")),
        "{found:?}"
    );
    let lone = base(svc).replace(
        "    reach: { home: any }\n",
        "    admin: read\n    admin_from: [10.0.0.0/8]\n",
    );
    let found = diag_paths(&lone);
    assert!(
        !found
            .iter()
            .any(|(c, _)| c == "unused-visitor" || c == "admin-shared"),
        "{found:?}"
    );
    let bad = lone.replace("10.0.0.0/8", "10.0.0.1/8");
    let found = diag_paths(&bad);
    assert!(
        found
            .iter()
            .any(|(c, p)| c == "invalid-source" && p.iter().any(|k| k == "admin_from")),
        "{found:?}"
    );
    let useless = base(svc).replace(
        "    reach: { home: any }\n",
        "    reach: { home: any }\n    admin_from: [10.0.0.0/8]\n",
    );
    assert!(
        diag_paths(&useless)
            .iter()
            .any(|(c, _)| c == "unused-admin-from")
    );
}

#[test]
fn carriers_cover_semtypes() {
    use crate::config::model::{Bind, ClientEntry, Mode, Reach, Secrets, ServerConfig, Target};
    fn proof(s: &SemType) {
        match s {
            SemType::Text(_) => assert_carrier::<String, sem::Text>(),
            SemType::Integer(_) => assert_carrier::<u32, sem::Integer>(),
            SemType::Bool => assert_carrier::<bool, sem::Bool>(),
            SemType::Enum(_) => assert_carrier::<Mode, sem::Enum>(),
            SemType::Ip(_) => assert_carrier::<String, sem::Ip>(),
            SemType::Port => assert_carrier::<u16, sem::Port>(),
            SemType::IpPort(_) => assert_carrier::<Bind, sem::IpPort>(),
            SemType::PortRange(_) => assert_carrier::<String, sem::PortRange>(),
            SemType::Target(_) => assert_carrier::<Target, sem::Target>(),
            SemType::Host => assert_carrier::<String, sem::Host>(),
            SemType::ServerAddress => assert_carrier::<String, sem::ServerAddress>(),
            SemType::Cidr => assert_carrier::<String, sem::Cidr>(),
            SemType::CidrRule => assert_carrier::<String, sem::CidrRule>(),
            SemType::Duration(_) => assert_carrier::<u32, sem::Duration>(),
            SemType::Size(_) => assert_carrier::<u64, sem::Size>(),
            SemType::Rate => assert_carrier::<String, sem::Rate>(),
            SemType::Ref(_) => assert_carrier::<String, sem::Ref>(),
            SemType::RefList(_) => assert_carrier::<Vec<String>, sem::RefList>(),
            SemType::RefMap(_) => assert_carrier::<BTreeMap<String, Reach>, sem::RefMap>(),
            SemType::Name(_) => assert_carrier::<String, sem::Name>(),
            SemType::SecretRef(_) => assert_carrier::<Secrets, sem::SecretRef>(),
            SemType::Path(_) => assert_carrier::<String, sem::Path>(),
            SemType::UrlPath => assert_carrier::<String, sem::UrlPath>(),
            SemType::Object(_) => assert_carrier::<ServerConfig, sem::Object>(),
            SemType::Map(_) => assert_carrier::<BTreeMap<String, ClientEntry>, sem::Map>(),
            SemType::List(_) => assert_carrier::<Vec<String>, sem::List>(),
            SemType::Union(_) => assert_carrier::<Reach, sem::Union>(),
            SemType::Optional(_) | SemType::Unknown(_) => {}
        }
    }
    for tag in SemType::KNOWN_TAGS {
        let s: SemType =
            serde_json::from_value(serde_json::json!({ "t": tag })).expect("every tag decodes");
        proof(&s);
    }
}

#[test]
fn schema_matches_the_frozen_fixture() {
    let schema = server_schema();
    assert!(schema.hash.starts_with("b2s:"));
    let text = serde_json::to_string_pretty(&schema).unwrap() + "\n";
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/burrow-admin-proto/tests/fixtures/v0.1/schema.json");
    if std::env::var_os("BURROW_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        std::fs::write(&fixture, &text).unwrap();
    }
    let frozen = std::fs::read_to_string(&fixture)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        frozen == text,
        "the admin schema changed; review the change, then run with BURROW_UPDATE_FIXTURES=1 to record it in {}",
        fixture.display()
    );
}

#[test]
fn every_node_has_a_label_and_known_type() {
    let schema = server_schema();
    let mut nodes = Vec::new();
    walk(&schema.root, &mut Vec::new(), &mut nodes);
    for (p, n) in nodes.iter().skip(1) {
        if p.last().is_some_and(|k| k != "*") {
            assert!(!n.label.is_empty(), "{p:?} has no label");
        }
        assert!(!matches!(n.sem, SemType::Unknown(_)), "{p:?}");
    }
    let back: Schema = serde_json::from_value(serde_json::to_value(&schema).unwrap()).unwrap();
    assert_eq!(back, schema);
}

#[test]
fn the_reduce_only_fields_are_exactly_what_the_grant_rule_reads() {
    let mut marked = server_schema().reduce_only_paths();
    marked.sort();
    let mut read: Vec<Vec<String>> = burrow_admin_logic::grant::FIELDS
        .iter()
        .map(|f| vec!["visitors".to_string(), "*".to_string(), f.to_string()])
        .collect();
    read.sort();
    assert_eq!(marked, read);
}
