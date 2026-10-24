use super::*;

fn plan(grants: &Grants, existing: &BTreeMap<String, Forward>) -> Plan {
    super::plan(grants, existing, false, false)
}

fn udp(mut g: Grants, names: &[&str]) -> Grants {
    for s in &mut g.services {
        if names.contains(&s.name.as_str()) {
            s.protocol = burrow_config::Protocol::Udp;
        }
    }
    g
}

#[test]
fn a_udp_service_gets_a_udp_forward_and_its_own_port_space() {
    let g = udp(
        grants(
            &[("dns", 53, 1), ("web", 10053, 1), ("game", 27015, 2)],
            &[],
        ),
        &["dns", "game"],
    );
    let p = plan(&g, &BTreeMap::new());
    assert_eq!(
        p.forwards["dns"],
        json!({ "service": "dns", "protocol": "udp", "local": 10053 })
    );
    assert_eq!(
        p.forwards["web"],
        json!({ "service": "web", "local": 10053 }),
        "a TCP and a UDP forward may share a port number"
    );
    assert_eq!(
        p.forwards["game"],
        json!({ "service": "game", "protocol": "udp", "local": "27015-27016" })
    );
}

#[test]
fn a_udp_forward_is_kept_while_granted_and_replaced_when_its_service_changes_protocol() {
    let mut existing = BTreeMap::new();
    existing.insert(
        "dns".to_string(),
        Forward::udp(at(5353), VisitTarget::service("dns")),
    );
    let kept = plan(&udp(grants(&[("dns", 53, 1)], &[]), &["dns"]), &existing);
    assert!(!kept.changed());
    assert_eq!(
        kept.forwards["dns"],
        json!({ "service": "dns", "protocol": "udp", "local": 5353 })
    );
    let now_tcp = plan(&grants(&[("dns", 53, 1)], &[]), &existing);
    assert_eq!(now_tcp.removed, vec!["dns"]);
    assert_eq!(now_tcp.added, vec!["dns"]);
    assert_eq!(
        now_tcp.forwards["dns"],
        json!({ "service": "dns", "local": 5353 }),
        "the forward keeps its local port in the other protocol when it is free"
    );
    let mut tcp = BTreeMap::new();
    tcp.insert(
        "dns".to_string(),
        Forward::single(at(5353), VisitTarget::service("dns")),
    );
    tcp.insert(
        "other".to_string(),
        Forward::udp(at(5353), VisitTarget::service("other")),
    );
    let g = udp(
        grants(&[("dns", 53, 1), ("other", 53, 1)], &[]),
        &["dns", "other"],
    );
    let busy = plan(&g, &tcp);
    assert_eq!(
        busy.forwards["dns"],
        json!({ "service": "dns", "protocol": "udp", "local": 5354 }),
        "a port the other protocol already uses moves to the next free one"
    );
}

#[test]
fn an_old_server_listing_tcp_only_keeps_every_udp_forward() {
    let mut existing = BTreeMap::new();
    existing.insert(
        "dns".to_string(),
        Forward::udp(at(5353), VisitTarget::service("dns")),
    );
    existing.insert(
        "db".to_string(),
        Forward::single(at(5432), VisitTarget::service("db")),
    );
    let old = super::plan(&grants(&[], &[]), &existing, false, true);
    assert_eq!(old.removed, vec!["db"]);
    assert_eq!(
        old.forwards["dns"],
        json!({ "service": "dns", "protocol": "udp", "local": 5353 })
    );
}

#[test]
fn free_ports_for_udp_forwards_are_probed_over_udp() {
    let taken = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let tcp = std::net::TcpListener::bind(("127.0.0.1", port)).ok();
    let g = udp(grants(&[("dns", port, 1)], &[]), &["dns"]);
    let p = super::plan(&g, &BTreeMap::new(), true, false);
    assert_ne!(
        p.forwards["dns"]["local"],
        json!(port),
        "the UDP port is in use"
    );
    drop(taken);
    let p = super::plan(&g, &BTreeMap::new(), true, false);
    assert_eq!(
        p.forwards["dns"]["local"],
        json!(port),
        "a TCP listener on the same number does not matter to a UDP forward"
    );
    drop(tcp);
}

#[test]
fn a_forward_to_part_of_a_range_is_kept_and_client_ranges_stay_ranges() {
    let mut existing = BTreeMap::new();
    existing.insert(
        "few".to_string(),
        Forward {
            ports: 3,
            ..Forward::single(at(40000), VisitTarget::service("game"))
        },
    );
    existing.insert(
        "lan".to_string(),
        Forward {
            ports: 11,
            ..Forward::single(
                at(30000),
                VisitTarget::Client {
                    client: "home".into(),
                    target: "127.0.0.1:30000".into(),
                },
            )
        },
    );
    let p = plan(&grants(&[("game", 40000, 11)], &["home"]), &existing);
    assert!(!p.changed(), "{:?} {:?}", p.added, p.removed);
    assert_eq!(
        p.forwards["lan"],
        json!({ "client": "home", "target": "127.0.0.1:30000-30010", "local": "30000-30010" })
    );
}
use crate::wire::protocol::{GrantedClient, GrantedService};
use std::net::SocketAddr;

fn grants(services: &[(&str, u16, u16)], clients: &[&str]) -> Grants {
    Grants {
        services: services
            .iter()
            .map(|(n, port, ports)| GrantedService {
                name: n.to_string(),
                protocol: burrow_config::Protocol::Tcp,
                port: *port,
                ports: *ports,
            })
            .collect(),
        clients: clients
            .iter()
            .map(|c| GrantedClient {
                name: c.to_string(),
                reach: vec![],
                allow: Some(vec!["127.0.0.0/8".into()]),
            })
            .collect(),
    }
}

#[test]
fn a_kept_forward_outside_the_current_reach_is_removed() {
    let mut g = grants(&[], &["home"]);
    g.clients[0].reach = vec!["127.0.0.0/8:41900-41999".into()];
    let client = |target: &str| {
        Forward::single(
            at(42222),
            VisitTarget::Client {
                client: "home".into(),
                target: target.into(),
            },
        )
    };
    let mut existing = BTreeMap::new();
    existing.insert("inside".to_string(), client("127.0.0.1:41950"));
    existing.insert("outside".to_string(), client("127.0.0.1:41800"));
    existing.insert("named".to_string(), client("db.lan:41800"));
    existing.insert("socket".to_string(), client("unix:/run/x.sock"));
    let p = plan(&g, &existing);
    assert_eq!(p.removed, ["outside", "socket"]);
    assert!(p.forwards.contains_key("inside") && p.forwards.contains_key("named"));
    g.clients[0].reach.push("a-rule-from-a-newer-server".into());
    assert!(plan(&g, &existing).removed.is_empty());
}

#[test]
fn a_local_port_already_in_use_is_skipped() {
    let busy = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = busy.local_addr().unwrap().port();
    let g = grants(&[("db", port, 1)], &[]);
    let probed = super::plan(&g, &BTreeMap::new(), true, false);
    assert_ne!(probed.forwards["db"]["local"], port);
    let p = plan(&g, &BTreeMap::new());
    assert_eq!(p.forwards["db"]["local"], port);
}

#[test]
fn grants_that_do_not_fit_are_left_out_instead_of_hanging() {
    let p = plan(
        &grants(
            &[("a", 30000, 30000), ("b", 31000, 30000), ("c", 5432, 1)],
            &["home"],
        ),
        &BTreeMap::new(),
    );
    assert_eq!(p.unplaced, vec!["b"]);
    assert_eq!(p.forwards["a"]["local"], "30000-59999");
    assert_eq!(p.forwards["c"]["local"], 5432);
    assert_eq!(p.forwards["home"]["local"], 1080);
    let many: Vec<(String, u16, u16)> = (0..70).map(|i| (format!("s{i}"), 20000, 1000)).collect();
    let many: Vec<(&str, u16, u16)> = many.iter().map(|(n, p, c)| (n.as_str(), *p, *c)).collect();
    let p = plan(&grants(&many, &[]), &BTreeMap::new());
    assert_eq!(p.forwards.len() + p.unplaced.len(), 70);
    assert!(!p.unplaced.is_empty());
    let mut seen = std::collections::HashSet::new();
    for v in p.forwards.values() {
        let (a, b) = v["local"].as_str().unwrap().split_once('-').unwrap();
        let (a, b): (u32, u32) = (a.parse().unwrap(), b.parse().unwrap());
        assert!(a >= 1024 && b <= 65535 && b - a == 999);
        assert!((a..=b).all(|p| seen.insert(p)), "overlap at {a}");
    }
}

fn at(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

#[test]
fn a_new_config_gets_a_forward_for_each_grant() {
    let p = plan(
        &grants(
            &[("db", 5432, 1), ("ssh", 22, 1), ("game", 40000, 11)],
            &["home"],
        ),
        &BTreeMap::new(),
    );
    assert_eq!(p.forwards["db"], json!({ "service": "db", "local": 5432 }));
    assert_eq!(
        p.forwards["ssh"],
        json!({ "service": "ssh", "local": 10022 })
    );
    assert_eq!(
        p.forwards["game"],
        json!({ "service": "game", "local": "40000-40010" })
    );
    assert_eq!(
        p.forwards["home"],
        json!({ "client": "home", "socks": true, "local": 1080 })
    );
    assert_eq!(p.added, vec!["db", "ssh", "game", "home"]);
    assert!(p.removed.is_empty());
}

#[test]
fn an_update_keeps_your_ports_adds_new_grants_and_drops_revoked_ones() {
    let mut existing = BTreeMap::new();
    existing.insert(
        "mydb".to_string(),
        Forward::single(at(6543), VisitTarget::service("db")),
    );
    existing.insert(
        "old".to_string(),
        Forward::single(at(7000), VisitTarget::service("gone")),
    );
    existing.insert(
        "ssh-home".to_string(),
        Forward::single(
            at(2222),
            VisitTarget::Client {
                client: "home".into(),
                target: "127.0.0.1:22".into(),
            },
        ),
    );
    let p = plan(
        &grants(&[("db", 5432, 1), ("web", 6543, 1)], &["home", "office"]),
        &existing,
    );
    assert_eq!(
        p.forwards["mydb"],
        json!({ "service": "db", "local": 6543 })
    );
    assert_eq!(
        p.forwards["web"],
        json!({ "service": "web", "local": 6544 }),
        "6543 is already yours"
    );
    assert_eq!(
        p.forwards["ssh-home"],
        json!({ "client": "home", "target": "127.0.0.1:22", "local": 2222 })
    );
    assert!(
        !p.forwards.contains_key("home"),
        "home is already reached through ssh-home"
    );
    assert_eq!(
        p.forwards["office"],
        json!({ "client": "office", "socks": true, "local": 1080 })
    );
    assert_eq!(p.added, vec!["web", "office"]);
    assert_eq!(p.removed, vec!["old"]);
}

#[test]
fn nothing_changes_when_the_grants_did_not() {
    let first = plan(&grants(&[("db", 5432, 1)], &["home"]), &BTreeMap::new());
    let mut existing = BTreeMap::new();
    existing.insert(
        "db".to_string(),
        Forward::single(at(5432), VisitTarget::service("db")),
    );
    existing.insert("home".to_string(), Forward::socks(at(1080), "home".into()));
    let again = plan(&grants(&[("db", 5432, 1)], &["home"]), &existing);
    assert!(!again.changed());
    assert_eq!(again.forwards, first.forwards);
    assert_eq!(again.allows["home"], Some(vec!["127.0.0.0/8".to_string()]));
}

#[test]
fn names_stay_unique_and_a_range_larger_than_granted_is_replaced() {
    let mut existing = BTreeMap::new();
    existing.insert(
        "game".to_string(),
        Forward {
            ports: 15,
            ..Forward::single(at(40000), VisitTarget::service("game"))
        },
    );
    let p = plan(
        &grants(&[("game", 40000, 11), ("home", 22, 1)], &["home"]),
        &existing,
    );
    assert_eq!(p.removed, vec!["game"]);
    assert_eq!(
        p.forwards["game"],
        json!({ "service": "game", "local": "40000-40010" })
    );
    assert_eq!(
        p.forwards["home"],
        json!({ "service": "home", "local": 10022 })
    );
    assert_eq!(
        p.forwards["home-socks"],
        json!({ "client": "home", "socks": true, "local": 1080 })
    );
}
