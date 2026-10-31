mod layout;
mod links;
mod view;

pub(crate) use layout::{Chart, Node, layout};
pub(crate) use view::FlowModal;

pub const PITCH: f64 = 54.0;
pub const TOP: f64 = 26.0;
pub const WIDTH: f64 = 1080.0;
const COLS: [(f64, f64); 5] = [
    (0.0, 180.0),
    (220.0, 190.0),
    (450.0, 140.0),
    (630.0, 190.0),
    (860.0, 220.0),
];
const LABELS: [&str; 5] = ["Sources", "Public ports", "Server", "Clients", "Targets"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Healthy,
    Degraded,
    Down,
    Offline,
    Unknown,
}

impl Health {
    fn stroke(self) -> &'static str {
        match self {
            Health::Healthy => "stroke-success/55",
            Health::Degraded => "stroke-warning/80",
            Health::Down => "stroke-danger/85",
            Health::Offline | Health::Unknown => "stroke-text-faint",
        }
    }

    fn node(self) -> &'static str {
        match self {
            Health::Degraded => "fill-warning-weak stroke-warning",
            Health::Down => "fill-danger-weak stroke-danger",
            _ => "fill-surface stroke-border-strong dark:fill-surface-3",
        }
    }
}

fn fit(text: &str, room: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= room {
        return text.to_string();
    }
    let keep = room.saturating_sub(1);
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::layout::weight;
    use super::*;
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
        let c = layout(&doc, Some(&st), &Rates::default(), None);
        assert_eq!(c.problems, 0);
        let target = c.nodes.iter().find(|n| n.key == "target:own").unwrap();
        assert_eq!(target.health, Health::Healthy);
        assert!(c.nodes.iter().all(|n| n.key != "client:self"));
        assert_eq!(c.lines.len(), 3, "source, port to server, server to target");
        let direct = &c.lines[2].d;
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
        st.visitors.push(
            burrow_admin_proto::serde_json::from_value(json!({"name": "v", "open": 0})).unwrap(),
        );
        let c = layout(&doc, Some(&st), &Rates::default(), Some("v"));
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
        let c = layout(&doc, Some(&status()), &Rates::default(), None);
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
        assert!(c.lines.iter().all(|l| l.d.starts_with('M')));
    }

    #[test]
    fn line_weight_grows_with_open_connections_and_stops_at_fifty() {
        assert_eq!(weight(0), 1.4);
        assert!((weight(10) - 2.8).abs() < 1e-9);
        assert_eq!(weight(50), weight(500));
    }

    #[test]
    fn without_status_nothing_is_called_a_problem() {
        let doc =
            json!({"clients": {"a": {}}, "services": {"web": {"client": "a", "target": "x:80"}}});
        let c = layout(&doc, None, &Rates::default(), None);
        assert_eq!(c.problems, 0);
        assert!(c.nodes.iter().all(|n| !n.problem));
    }
}
