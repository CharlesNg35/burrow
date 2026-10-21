#![cfg(test)]

use burrow_admin_logic::check::{check_in, refs_exist};
use burrow_admin_logic::relations::entity_problems;
use burrow_admin_logic::tree::{entity_names, inner};
use burrow_admin_proto::Path;
use burrow_admin_proto::msg::AdminLevel;
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::schema::{Node, SemType};
use burrow_config::{
    ClientEntry, HealthCheck, Limits, MetricsConfig, SelfEntry, ServerConfig, ServiceEntry,
    TcpOptions, VisitorEntry,
};
use serde_json::{Value, json};

use super::tests::{core_for, session, world};
use super::{plan, text};

enum Cases {
    Values(Vec<Case>),
    NotInTheServerSchema,
    Locked,
    ServerGenerated,
    Container,
}

struct Case {
    path: Path,
    item: bool,
    value: Value,
}

fn at(path: Path, values: &[Value]) -> Vec<Case> {
    values
        .iter()
        .map(|v| Case {
            path: path.clone(),
            item: false,
            value: v.clone(),
        })
        .collect()
}

fn items(path: Path, values: &[Value]) -> Vec<Case> {
    values
        .iter()
        .map(|v| Case {
            path: path.clone(),
            item: true,
            value: v.clone(),
        })
        .collect()
}

fn cases(sem: &SemType) -> Cases {
    match sem {
        SemType::IpPort(_) => {
            let mut c = at(
                ServiceEntry::BIND.of("web"),
                &[
                    json!(7841),
                    json!("7841"),
                    json!("127.0.0.1:7841"),
                    json!("[::1]:7841"),
                    json!("0.0.0.0:7841"),
                    json!(70000),
                    json!(0),
                    json!("abc"),
                    json!("127.0.0.1"),
                    json!("1.2.3.4:99999"),
                    json!("7841-"),
                    json!("host.example:7841"),
                    json!("127.0.0.1:7841:1"),
                    json!(""),
                ],
            );
            c.extend(at(
                ServerConfig::METRICS.then(MetricsConfig::LISTEN).path(),
                &[
                    json!("127.0.0.1:9100"),
                    json!(9100),
                    json!("127.0.0.1:9100-9101"),
                    json!("[::1]:9100"),
                ],
            ));
            Cases::Values(c)
        }
        SemType::Target(_) => Cases::Values(at(
            ServiceEntry::TARGET.of("web"),
            &[
                json!(3001),
                json!("3001"),
                json!("127.0.0.1:3001"),
                json!("db.local:5432"),
                json!("unix:/run/app.sock"),
                json!("[::1]:22"),
                json!(70000),
                json!("127.0.0.1:"),
                json!("unix:relative"),
                json!(""),
                json!("http://x"),
                json!(0),
            ],
        )),
        SemType::Integer(_) => {
            let mut c = at(
                ClientEntry::MAX_CONNECTIONS.of("home"),
                &[
                    json!(1),
                    json!(100),
                    json!(0),
                    json!(-1),
                    json!("x"),
                    json!(1.5),
                    json!("5"),
                ],
            );
            c.extend(at(
                ServerConfig::LIMITS
                    .then(Limits::MAX_CONNECTIONS_PER_CLIENT)
                    .path(),
                &[json!(10), json!(0), json!(4_294_967_296_u64)],
            ));
            c.extend(at(
                ServiceEntry::HEALTH_CHECK
                    .then(HealthCheck::FAILURES)
                    .of("web"),
                &[json!(2), json!(100), json!(101), json!(0)],
            ));
            Cases::Values(c)
        }
        SemType::Duration(_) => {
            let mut c = at(
                ServiceEntry::HEALTH_CHECK
                    .then(HealthCheck::INTERVAL_SECS)
                    .of("web"),
                &[
                    json!(10),
                    json!(1),
                    json!(3600),
                    json!(0),
                    json!(3601),
                    json!(-5),
                    json!("x"),
                ],
            );
            c.extend(at(
                ServerConfig::TCP.then(TcpOptions::KEEPALIVE_SECS).path(),
                &[json!(0), json!(60), json!(32767), json!(32768), json!(-1)],
            ));
            Cases::Values(c)
        }
        SemType::Bool => Cases::Values(at(
            ServerConfig::TCP.then(TcpOptions::NODELAY).path(),
            &[json!(true), json!(false), json!("yes"), json!(1)],
        )),
        SemType::Enum(_) => {
            let mut c = at(
                ServiceEntry::PROTOCOL.of("web"),
                &[json!("tcp"), json!("udp"), json!("TCP"), json!("sctp")],
            );
            c.extend(at(
                ServiceEntry::PROXY_PROTOCOL.of("web"),
                &[json!("v1"), json!("v2"), json!("v3")],
            ));
            c.extend(at(
                ServiceEntry::MODE.of("web"),
                &[json!("failover"), json!("balance"), json!("random")],
            ));
            c.extend(at(
                VisitorEntry::ADMIN.of("lap"),
                &[json!("read"), json!("write"), json!("admin")],
            ));
            Cases::Values(c)
        }
        SemType::Rate => Cases::Values(at(
            ServiceEntry::BANDWIDTH.of("web"),
            &[
                json!("10mbit"),
                json!("10 Mbit/s"),
                json!("1MB/s"),
                json!("500kbit"),
                json!("fast"),
                json!("-1mbit"),
                json!("0"),
                json!("0mbit"),
                json!(""),
                json!(10),
            ],
        )),
        SemType::Ref(_) => Cases::Values(at(
            ServiceEntry::CLIENT.of("web"),
            &[
                json!("home"),
                json!("office"),
                json!("nobody"),
                json!(""),
                json!(3),
            ],
        )),
        SemType::RefList(_) => {
            let mut c = at(
                ServiceEntry::VISITORS.of("web"),
                &[
                    json!(["lap"]),
                    json!([]),
                    json!(["lap", "lap"]),
                    json!(["ghost"]),
                    json!("lap"),
                ],
            );
            c.extend(at(
                ServiceEntry::CLIENTS.of("web"),
                &[json!(["home", "office"]), json!(["home", "home"])],
            ));
            Cases::Values(c)
        }
        SemType::ServerAddress => Cases::Values(at(
            ServerConfig::PUBLIC_ADDRESS.path(),
            &[
                json!("tunnel.example.com"),
                json!("1.2.3.4"),
                json!("tunnel.example.com:7900"),
                json!("[::1]"),
                json!("[::1]:7900"),
                json!("::1"),
                json!("wss://x.example.com/burrow"),
                json!(""),
                json!("http://x"),
                json!("a b"),
                json!("tunnel.example.com:0"),
            ],
        )),
        SemType::Cidr => Cases::Values(items(
            ServiceEntry::ALLOW_FROM.of("web"),
            &[
                json!("10.0.0.0/8"),
                json!("203.0.113.7"),
                json!("::/0"),
                json!("::ffff:10.0.0.0/104"),
                json!("10.0.0.1/8"),
                json!("10.0.0.0/33"),
                json!("x"),
                json!("10.0.0.0/8:22"),
            ],
        )),
        SemType::CidrRule => Cases::Values(items(
            ServerConfig::SELF.then(SelfEntry::ALLOW).path(),
            &[
                json!("10.0.0.0/8:22"),
                json!("127.0.0.1:5432"),
                json!("10.0.0.0/24"),
                json!("unix:/run/x.sock"),
                json!("10.0.0.0/8:0"),
                json!("10.0.0.0/8:22-21"),
                json!("x"),
                json!("10.0.0.1/8"),
            ],
        )),
        SemType::Union(_) => {
            let mut c = at(
                ServiceEntry::HEALTH_CHECK.of("web"),
                &[
                    json!(true),
                    json!(false),
                    json!({"interval_secs": 5}),
                    json!("sometimes"),
                ],
            );
            c.extend(at(
                VisitorEntry::REACH.entry_of("lap", "home"),
                &[
                    json!("any"),
                    json!("10.0.0.0/8:22"),
                    json!(["10.0.0.0/8:22", "127.0.0.1:80"]),
                    json!("all"),
                    json!(["x"]),
                ],
            ));
            Cases::Values(c)
        }
        SemType::UrlPath | SemType::Path(_) => Cases::Locked,
        SemType::SecretRef(_) => Cases::ServerGenerated,
        SemType::Object(_)
        | SemType::Map(_)
        | SemType::RefMap(_)
        | SemType::List(_)
        | SemType::Optional(_) => Cases::Container,
        SemType::Text(_)
        | SemType::Ip(_)
        | SemType::Port
        | SemType::PortRange(_)
        | SemType::Host
        | SemType::Size(_)
        | SemType::Name(_)
        | SemType::Unknown(_) => Cases::NotInTheServerSchema,
    }
}

fn walk<'a>(n: &'a Node, out: &mut Vec<&'a SemType>) {
    out.push(&n.sem);
    match &n.sem {
        SemType::Object(o) => o.fields.iter().for_each(|f| walk(&f.node, out)),
        SemType::Optional(o) => walk(&o.inner, out),
        SemType::Map(m) => walk(&m.value, out),
        SemType::RefMap(m) => walk(&m.value, out),
        SemType::List(l) => walk(&l.item, out),
        SemType::Union(u) => u.branches.iter().for_each(|b| walk(b, out)),
        _ => {}
    }
}

fn base() -> (super::tests::World, String) {
    let w = world();
    let (office, lap) = (
        crate::secret::Secret::generate(),
        crate::secret::Secret::generate(),
    );
    let text = w.text.replace(
        "\nvisitors:",
        &format!(
            "  office:\n    secret: {}\n\nvisitors:\n  lap:\n    secret: {}",
            office.encode(),
            lap.encode()
        ),
    );
    (w, text)
}

fn server_ok(core: &super::Core, s: &super::Session, base: &str, case: &Case) -> bool {
    let value = if case.item {
        json!([case.value.clone()])
    } else {
        case.value.clone()
    };
    let ops = [Op::Set {
        path: case.path.clone(),
        value,
    }];
    plan::compute(core, s, &text::rev(base), &ops)
        .map(|c| c.plan.valid)
        .unwrap_or(false)
}

fn node_at<'a>(n: &'a Node, path: &[String]) -> Option<&'a Node> {
    let Some((first, rest)) = path.split_first() else {
        return Some(n);
    };
    match &n.sem {
        SemType::Optional(o) => node_at(&o.inner, path),
        SemType::Union(u) => u.branches.iter().find_map(|b| node_at(b, path)),
        _ => node_at(n.at(std::slice::from_ref(first))?, rest),
    }
}

fn ui_ok(core: &super::Core, base: &str, case: &Case) -> bool {
    let schema = core.schema();
    let mut path = case.path.clone();
    if case.item {
        path.push("0".into());
    }
    let Some(node) = node_at(&schema.root, &path) else {
        panic!("no schema node at {path:?}");
    };
    let doc = plan::check_text(core, base).raw.unwrap_or(Value::Null);
    let names = match &inner(node).sem {
        SemType::Ref(r) => entity_names(schema, &doc, &r.entity),
        SemType::RefList(r) => entity_names(schema, &doc, &r.entity),
        _ => Vec::new(),
    };
    let mut after = doc.clone();
    let value = if case.item {
        json!([case.value.clone()])
    } else {
        case.value.clone()
    };
    burrow_admin_proto::value::get_mut(&mut after, &case.path[..case.path.len() - 1])
        .and_then(Value::as_object_mut)
        .map(|m| m.insert(case.path[case.path.len() - 1].clone(), value));
    let entity_ok = match case.path.as_slice() {
        [section, name, ..] => {
            let node = schema.root.at(&[section.clone(), "*".to_string()]);
            let value = burrow_admin_proto::value::get(&after, &[section.clone(), name.clone()]);
            match (node, value) {
                (Some(n), Some(v)) => entity_problems(n, v).is_empty(),
                _ => true,
            }
        }
        _ => true,
    };
    check_in(node, &case.value, &doc).is_ok()
        && refs_exist(node, &case.value, &names).is_ok()
        && entity_ok
}

#[test]
fn the_ui_and_the_server_agree_on_every_value_of_every_semantic_type() {
    let (w, text) = base();
    let core = core_for(&text);
    assert!(
        plan::check_text(&core, &text).valid,
        "the base config loads"
    );
    let s = session(&w, AdminLevel::Write);
    let mut used = Vec::new();
    walk(&core.schema().root, &mut used);
    let mut checked = std::collections::BTreeSet::new();
    let mut disagree = Vec::new();
    for sem in used {
        let tag = sem.tag().to_string();
        if !checked.insert(tag.clone()) {
            continue;
        }
        let Cases::Values(list) = cases(sem) else {
            continue;
        };
        let (mut yes, mut no) = (0, 0);
        for case in &list {
            let (ui, srv) = (ui_ok(&core, &text, case), server_ok(&core, &s, &text, case));
            if srv {
                yes += 1;
            } else {
                no += 1;
            }
            if ui != srv {
                disagree.push(format!(
                    "{tag} at {}: {} ui={ui} server={srv}",
                    case.path.join("."),
                    case.value
                ));
            }
        }
        assert!(yes > 0 && no > 0, "{tag} needs accepted and refused cases");
    }
    assert!(disagree.is_empty(), "{}", disagree.join("\n"));
}

#[test]
fn the_admin_ui_accepts_exactly_the_public_addresses_validate_accepts() {
    let node = Node::new(SemType::ServerAddress);
    let listen: std::net::SocketAddr = "0.0.0.0:7835".parse().unwrap();
    let ws = burrow_syntax::address::WsShape {
        listen: "127.0.0.1:8080".parse().unwrap(),
        path: "/burrow",
        tls: false,
    };
    for given in [
        "tunnel.example.com",
        "tunnel.example.com:9000",
        "203.0.113.5",
        "203.0.113.5:7835",
        "[2001:db8::1]",
        "[2001:db8::1]:7835",
        "2001:db8::1",
        "ws://tunnel.example.com",
        "ws://tunnel.example.com:8080/burrow",
        "wss://tunnel.example.com/burrow",
        "wss://[2001:db8::1]/burrow",
        "wss://[2001:db8::1]:8443/burrow",
        "wss://2001:db8::1/burrow",
        "http://tunnel.example.com",
        "tunnel.example.com:",
        "0.0.0.0",
        "[::]:7835",
        "user@tunnel.example.com",
        "",
    ] {
        let mut notes = Vec::new();
        let server = burrow_syntax::address::client_address(
            given,
            Some(listen),
            Some(ws),
            "clients",
            &mut notes,
        )
        .is_ok();
        let ui =
            burrow_admin_logic::check::check(&node, &burrow_admin_proto::serde_json::json!(given))
                .is_ok();
        assert_eq!(ui, server, "{given:?}");
    }
    for (given, accepted) in [
        ("tunnel.example.com", true),
        ("[2001:db8::1]", true),
        ("2001:db8::1", false),
        ("wss://[2001:db8::1]/burrow", true),
    ] {
        let mut notes = Vec::new();
        let server =
            burrow_syntax::address::client_address(given, None, Some(ws), "clients", &mut notes)
                .is_ok();
        assert_eq!(server, accepted, "{given:?} on a WebSocket-only server");
    }
}
