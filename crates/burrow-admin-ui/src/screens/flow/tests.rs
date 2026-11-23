use super::layout::weight;
use super::*;

fn q(text: &str) -> Focus {
    Focus {
        query: text.into(),
        ..Focus::default()
    }
}
use burrow_admin_logic::rates::Rates;
use burrow_admin_proto::status::Status;

#[test]
fn long_text_is_cut_in_the_middle() {
    assert_eq!(fit("short", 10), "short");
    assert_eq!(fit("unix:/home/ubuntu/app.sock", 12), "unix:/….sock");
    assert_eq!(fit("unix:/home/ubuntu/app.sock", 12).chars().count(), 12);
}
use burrow_admin_proto::serde_json::json;
use burrow_admin_proto::status::{ClientStatus, ServiceStatus};

fn status() -> Status {
    let mut st: Status = burrow_admin_proto::serde_json::from_value(json!({
        "version": "0.1.0", "uptime_secs": 10, "listening": ["0.0.0.0:7835"], "handshakes_refused": 0, "udp_peers": 0,
        "clients": [], "services": [], "visitors": []
    }))
    .unwrap();
    st.clients.push(ClientStatus {
        name: "a".into(),
        connected: true,
        connected_secs: Some(5),
        from: Some("1.2.3.4".into()),
        open: 3,
        idle_channels: 2,
        left_secs_ago: None,
        local: false,
        ..ClientStatus::default()
    });
    st.clients.push(ClientStatus {
        name: "b".into(),
        connected: false,
        connected_secs: None,
        from: None,
        open: 0,
        idle_channels: 0,
        left_secs_ago: Some(7200),
        local: false,
        ..ClientStatus::default()
    });
    let mut svc: ServiceStatus = burrow_admin_proto::serde_json::from_value(
        json!({"name": "web", "protocol": "tcp", "target": "x:80", "clients": ["a"]}),
    )
    .unwrap();
    svc.open = 60;
    st.services.push(svc);
    st
}

#[test]
fn a_service_on_this_server_goes_from_the_server_straight_to_its_target() {
    let mut st = status();
    st.clients.push(ClientStatus {
        name: "self".into(),
        connected: true,
        local: true,
        ..ClientStatus::default()
    });
    let doc = json!({
        "clients": {},
        "services": {"own": {"client": "self", "bind": "0.0.0.0:8080", "target": "127.0.0.1:3000"}}
    });
    let c = layout(&doc, Some(&st), &Rates::default(), None, &Focus::default());
    assert_eq!(c.problems, 0);
    let target = c.nodes.iter().find(|n| n.key == "target:own").unwrap();
    assert_eq!(target.health, Health::Healthy);
    assert!(c.nodes.iter().all(|n| n.key != "client:self"));
    assert_eq!(c.lines.len(), 3, "source, port to server, server to target");
    let direct = &c.lines[2].path.d;
    assert!(
        direct.contains(&format!(" {:.1},", COLS[4].0)),
        "the line ends at the target column: {direct}"
    );
}

#[test]
fn the_source_nodes_say_their_counts_without_cutting_them() {
    let doc = json!({
        "clients": {"a": {}},
        "visitors": {"v": {}},
        "services": {
            "web": {"client": "a", "bind": "0.0.0.0:443", "target": "x:80"},
            "lo": {"client": "a", "bind": "127.0.0.1:3000", "target": "z:3000"},
            "db": {"client": "a", "target": "y:5432", "visitors": ["v"]}
        }
    });
    let mut st = status();
    st.visitors
        .push(burrow_admin_proto::serde_json::from_value(json!({"name": "v", "open": 0})).unwrap());
    let c = layout(
        &doc,
        Some(&st),
        &Rates::default(),
        Some("v"),
        &Focus::default(),
    );
    for n in c.nodes.iter().filter(|n| n.col == 0) {
        let room = ((COLS[0].1 - 18.0) / 5.4) as usize;
        for line in n.sub.lines() {
            assert!(line.chars().count() <= room, "{line:?} is cut");
        }
    }
    let visitors = c.nodes.iter().find(|n| n.title == "Visitors").unwrap();
    assert_eq!(
        visitors.sub, "1 in use",
        "your own admin session counts, as on the overview"
    );
}

#[test]
fn the_flow_marks_down_services_and_away_clients_as_problems() {
    let doc = json!({
        "clients": {"a": {}, "b": {}},
        "services": {
            "web": {"client": "a", "bind": "0.0.0.0:443", "target": "x:80"},
            "db": {"client": "b", "target": "y:5432"},
            "lo": {"client": "a", "bind": "127.0.0.1:3000", "target": "z:3000"}
        }
    });
    let c = layout(
        &doc,
        Some(&status()),
        &Rates::default(),
        None,
        &Focus::default(),
    );
    assert_eq!(c.services, 3);
    assert_eq!(c.problems, 2, "db is down and b is away");
    let order: Vec<&str> = c
        .nodes
        .iter()
        .filter(|n| n.col == 1)
        .map(|n| n.title.as_str())
        .collect();
    assert_eq!(
        order,
        ["lo", "web", "db"],
        "public first, then private, each by name"
    );
    let internet = c.nodes.iter().find(|n| n.key == "internet").unwrap();
    assert_eq!(internet.sub, "1 public · 0 refused");
    assert_eq!(internet.right, "1 local");
    let db = c.nodes.iter().find(|n| n.key == "target:db").unwrap();
    assert_eq!(db.health, Health::Down);
    assert_eq!(db.sub, "no client online");
    let b = c.nodes.iter().find(|n| n.key == "client:b").unwrap();
    assert!(b.dashed && b.problem);
    assert_eq!(b.badge.as_deref(), Some("offline"));
    let web = c.nodes.iter().find(|n| n.key == "port:web").unwrap();
    assert!(web.right.is_empty(), "the bind sits on line 2");
    assert!(web.sub.starts_with(":443 · "), "{}", web.sub);
    assert_eq!(c.height, TOP + 3.0 * PITCH);
    assert!(c.lines.iter().all(|l| l.path.d.starts_with('M')));
}

#[test]
fn a_search_narrows_the_services_and_the_clients_drawn() {
    let doc = json!({
        "clients": {"a": {}, "b": {}, "c": {}},
        "services": {
            "web": {"client": "a", "bind": "0.0.0.0:443", "target": "x:80"},
            "db": {"client": "b", "target": "y:5432"},
            "lo": {"client": "a", "bind": "127.0.0.1:3000", "target": "z:3000"}
        }
    });
    let st = status();
    let titles = |c: &Chart, col: usize| -> Vec<String> {
        c.nodes
            .iter()
            .filter(|n| n.col == col)
            .map(|n| n.title.clone())
            .collect()
    };
    let all = layout(&doc, Some(&st), &Rates::default(), None, &Focus::default());
    assert_eq!((all.services, all.total), (3, 3));
    assert_eq!(titles(&all, 3), ["a", "b", "c"]);
    let db = layout(&doc, Some(&st), &Rates::default(), None, &q(" DB "));
    assert_eq!((db.services, db.total), (1, 3));
    assert_eq!(titles(&db, 1), ["db"]);
    assert_eq!(titles(&db, 3), ["b"], "only the clients of what is drawn");
    let by_client = layout(&doc, Some(&st), &Rates::default(), None, &q("a"));
    assert_eq!(titles(&by_client, 1), ["lo", "web"]);
    assert_eq!(titles(&by_client, 3), ["a"]);
    let by_target = layout(&doc, Some(&st), &Rates::default(), None, &q("5432"));
    assert_eq!(titles(&by_target, 1), ["db"]);
    let none = layout(&doc, Some(&st), &Rates::default(), None, &q("nothing"));
    assert_eq!(none.services, 0);
    assert!(none.nodes.iter().any(|n| n.server));
}

fn many(n: usize) -> burrow_admin_proto::serde_json::Value {
    let mut services = burrow_admin_proto::serde_json::Map::new();
    for i in 0..n {
        services.insert(
            format!("s{i:04}"),
            json!({"client": "a", "target": format!("x:{i}"), "bind": format!("0.0.0.0:{}", 9000 + i)}),
        );
    }
    services.insert("down".into(), json!({"client": "b", "target": "y:1"}));
    services.insert(
        "half".into(),
        json!({"clients": ["a", "b"], "target": "y:2"}),
    );
    json!({"clients": {"a": {}, "b": {}}, "services": services})
}

fn keys_are_unique(c: &Chart) {
    let nodes: std::collections::HashSet<&str> = c.nodes.iter().map(|n| n.key.as_str()).collect();
    let lines: std::collections::HashSet<&str> = c.lines.iter().map(|l| l.key.as_str()).collect();
    assert_eq!(nodes.len(), c.nodes.len());
    assert_eq!(lines.len(), c.lines.len());
}

#[test]
fn a_small_chart_draws_every_service() {
    let c = layout(
        &many(150),
        Some(&status()),
        &Rates::default(),
        None,
        &Focus::default(),
    );
    keys_are_unique(&c);
    assert_eq!(c.nodes.iter().filter(|n| n.col == 1).count(), 152);
    assert!(c.nodes.iter().all(|n| !n.key.starts_with("bundle:")));
}

#[test]
fn a_large_chart_folds_each_client_but_keeps_the_problems_apart() {
    let doc = many(5000);
    let st = status();
    let c = layout(&doc, Some(&st), &Rates::default(), None, &Focus::default());
    keys_are_unique(&c);
    assert_eq!(c.services, 5002);
    let ports: Vec<&str> = c
        .nodes
        .iter()
        .filter(|n| n.col == 1)
        .map(|n| n.key.as_str())
        .collect();
    assert_eq!(ports, ["bundle:a", "port:down", "port:half"]);
    let bundle = c.nodes.iter().find(|n| n.key == "bundle:a").unwrap();
    assert_eq!(bundle.sub, "5,000 services · 1 down");
    assert_eq!(
        bundle.act,
        Some(Act::Fold {
            client: "a".into(),
            open: false
        })
    );
    assert!(c.nodes.iter().any(|n| n.key == "targets:a"));
    assert!(c.lines.iter().any(|l| l.key == "bundle:a/in:a"));
    let open = Focus {
        open: ["a".to_string()].into(),
        ..Focus::default()
    };
    let wide = layout(&doc, Some(&st), &Rates::default(), None, &open);
    keys_are_unique(&wide);
    assert_eq!(wide.nodes.iter().filter(|n| n.col == 1).count(), 5003);
    let head = wide.nodes.iter().find(|n| n.key == "bundle:a").unwrap();
    assert_eq!(head.right, "fold");
    assert!(wide.nodes.iter().all(|n| n.key != "targets:a"));
    let a = wide.nodes.iter().find(|n| n.key == "client:a").unwrap();
    assert_eq!(a.y, head.y, "the client starts at its unfolded block");
    assert!(a.stick.unwrap() > a.y + 4000.0 * PITCH);
    let b = wide.nodes.iter().find(|n| n.key == "client:b").unwrap();
    assert!(b.stick.is_none() && b.y > a.stick.unwrap());
    let to = wide.lines.iter().find(|l| l.key == "to:s0001:0").unwrap();
    let target = wide.nodes.iter().find(|n| n.key == "target:s0001").unwrap();
    assert!(to.stick.is_some());
    assert_eq!(to.path.top, target.y + 21.0, "kept by its target end");
    let net = wide.nodes.iter().find(|n| n.key == "internet").unwrap();
    assert_eq!(
        net.h, 42.0,
        "a tall source becomes a node that stays in view"
    );
    assert!(net.stick.unwrap() > net.y + 4000.0 * PITCH);
    let src = wide.lines.iter().find(|l| l.key == "src:s0001").unwrap();
    let port = wide.nodes.iter().find(|n| n.key == "port:s0001").unwrap();
    assert_eq!(src.path.top, port.y + 21.0, "kept by its port end");
    assert!(
        c.nodes
            .iter()
            .filter(|n| n.col != 0)
            .all(|n| n.stick.is_none()),
        "a folded chart keeps its clients spread as before"
    );
    let found = layout(&doc, Some(&st), &Rates::default(), None, &q("s004"));
    assert_eq!(found.services, 10);
    assert!(found.nodes.iter().any(|n| n.key == "port:s0049"));
    let again = layout(&doc, Some(&st), &Rates::default(), None, &Focus::default());
    assert_eq!(c, again);
}

#[test]
fn line_weight_grows_with_open_connections_and_stops_at_fifty() {
    assert_eq!(weight(0), 1.4);
    assert!((weight(10) - 2.8).abs() < 1e-9);
    assert_eq!(weight(50), weight(500));
}

#[test]
fn without_status_nothing_is_called_a_problem() {
    let doc = json!({"clients": {"a": {}}, "services": {"web": {"client": "a", "target": "x:80"}}});
    let c = layout(&doc, None, &Rates::default(), None, &Focus::default());
    assert_eq!(c.problems, 0);
    assert!(c.nodes.iter().all(|n| !n.problem));
}
