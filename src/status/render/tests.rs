use super::*;
use crate::metrics::{ClientStatus, HealthStatus, ServiceStatus, SetAside, VisitorStatus};

fn strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn sample() -> Status {
    Status {
        version: "9.9.9".into(),
        uptime_secs: 3 * 86400 + 4 * 3600,
        listening: vec!["clients on tcp 0.0.0.0:7835".into()],
        handshakes_refused: 2,
        udp_peers: 0,
        clients: vec![
            ClientStatus {
                name: "home".into(),
                connected: true,
                connected_secs: Some(2 * 3600 + 13 * 60),
                from: Some("81.2.3.4:51234".into()),
                open: 3,
                idle_channels: 4,
                left_secs_ago: None,
                local: false,
                ..ClientStatus::default()
            },
            ClientStatus {
                name: "office".into(),
                connected: false,
                connected_secs: None,
                from: None,
                open: 0,
                idle_channels: 0,
                left_secs_ago: Some(300),
                local: false,
                ..ClientStatus::default()
            },
        ],
        services: vec![
            ServiceStatus {
                name: "web".into(),
                protocol: "tcp".into(),
                bind: Some("0.0.0.0:443".into()),
                target: "127.0.0.1:8443".into(),
                clients: vec!["home".into(), "office".into()],
                mode: Some("balance".into()),
                bandwidth: Some(5_000_000),
                visitors: vec![],
                proxy_protocol: Some("v2".into()),
                filtered: true,
                open: 12,
                total: 153,
                refused: 1,
                bytes_in: 1_200_000_000,
                bytes_out: 8_400_000_000,
                health: vec![HealthStatus {
                    client: "home".into(),
                    up: true,
                    reason: None,
                }],
                set_aside: vec![],
                web: vec![],
                http: None,
            },
            ServiceStatus {
                name: "db".into(),
                protocol: "tcp".into(),
                bind: None,
                target: "127.0.0.1:5432".into(),
                clients: vec!["home".into()],
                mode: None,
                bandwidth: None,
                visitors: vec!["laptop".into()],
                proxy_protocol: None,
                filtered: false,
                open: 0,
                total: 0,
                refused: 0,
                bytes_in: 0,
                bytes_out: 0,
                health: vec![],
                set_aside: vec![],
                web: vec![],
                http: None,
            },
        ],
        visitors: vec![
            VisitorStatus {
                name: "laptop".into(),
                open: 1,
                total: 12,
                last_seen_secs: Some(0),
            },
            VisitorStatus {
                name: "phone".into(),
                open: 0,
                total: 3,
                last_seen_secs: Some(150),
            },
            VisitorStatus {
                name: "ops".into(),
                open: 0,
                total: 0,
                last_seen_secs: None,
            },
        ],
        web: None,
        certificates: Vec::new(),
        acme: None,
        local_ca: None,
    }
}

#[test]
fn plain_tables() {
    let text = render(&sample(), Paint::PLAIN, None);
    assert_eq!(
        text,
        "burrow 9.9.9  ·  up 3 d 4 h  ·  clients on tcp 0.0.0.0:7835

CLIENTS  1/2 connected
  NAME    STATE      SINCE           FROM            OPEN  IDLE
  home    connected  2 h 13 min      81.2.3.4:51234     3     4
  office  offline    left 5 min ago                     0     0

SERVICES
  NAME  PROTO  ADDRESS      TARGET          CLIENTS                  OPEN  TOTAL  REFUSED      IN     OUT
  web   tcp    0.0.0.0:443  127.0.0.1:8443  home up, office offline    12    153        1  1.2 GB  8.4 GB
  db    tcp    private      127.0.0.1:5432  home                        0      0        0     0 B     0 B
  web: balance, limit 5.0 MB/s, proxy protocol v2, source filter
  db: visitors laptop

VISITORS
  NAME    LAST USED        OPEN  TOTAL
  laptop  now                 1     12
  phone   2 min ago           0      3
  ops     not since start     0      0

2 refused handshakes since start (wrong secret, unknown name or limits)
"
    );
}

#[test]
fn a_websites_responses_are_counted_by_class() {
    let mut s = sample();
    s.services[0].http = Some(burrow_admin_proto::status::HttpCounts {
        responses: [0, 1203, 4, 17, 2],
    });
    s.services[1].http = Some(burrow_admin_proto::status::HttpCounts::default());
    let text = render(&s, Paint::PLAIN, None);
    assert!(
        text.contains("  web: balance, limit 5.0 MB/s, proxy protocol v2, source filter, web responses 2xx 1203, 3xx 4, 4xx 17, 5xx 2\n"),
        "{text}"
    );
    assert!(
        text.contains("  db: visitors laptop, no web responses yet\n"),
        "{text}"
    );
}

#[test]
fn this_server_is_listed_as_local_and_not_counted_as_a_client() {
    let mut s = sample();
    s.clients.push(ClientStatus {
        name: "self".into(),
        connected: true,
        connected_secs: Some(60),
        open: 2,
        local: true,
        ..ClientStatus::default()
    });
    let text = render(&s, Paint::PLAIN, None);
    assert!(text.contains("CLIENTS  1/2 connected"), "{text}");
    assert!(
        text.contains("  self    connected  1 min           this server        2     0"),
        "{text}"
    );
}

#[test]
fn a_client_at_its_limit_says_so_next_to_its_state() {
    let mut s = sample();
    s.clients[0].at_limit = true;
    let text = render(&s, Paint::PLAIN, None);
    let line = |name: &str| {
        text.lines()
            .find(|l| l.trim_start().starts_with(name))
            .unwrap()
            .to_string()
    };
    assert!(line("home").contains("connected, at limit"), "{text}");
    assert!(!line("office").contains("at limit"), "{text}");
    assert!(!render(&sample(), Paint::PLAIN, None).contains("at limit"));
}

#[test]
fn clients_set_aside_say_why() {
    let mut s = sample();
    let web = &mut s.services[0];
    web.set_aside = vec![SetAside {
        client: "home".into(),
        refused: true,
    }];
    let text = render(&s, Paint::PLAIN, None);
    assert!(text.contains("home refused, office offline"), "{text}");
    let web = &mut s.services[0];
    web.health.clear();
    web.set_aside[0].refused = false;
    let text = render(&s, Paint::PLAIN, None);
    assert!(text.contains("home failing, office "), "{text}");
    let web = &mut s.services[0];
    web.health = vec![HealthStatus {
        client: "home".into(),
        up: false,
        reason: None,
    }];
    let text = render(&s, Paint::PLAIN, None);
    assert!(text.contains("home down, office offline"), "{text}");
    assert!(!text.contains("home down:"), "{text}");
    s.services[0].health[0].reason = Some("answered 401 Unauthorized".into());
    let text = render(&s, Paint::PLAIN, None);
    assert!(
        text.contains("home down: answered 401 Unauthorized"),
        "{text}"
    );
    s.services[0].health[0].up = true;
    let text = render(&s, Paint::PLAIN, None);
    assert!(!text.contains("answered 401"), "{text}");
}

#[test]
fn colors_keep_the_columns_aligned() {
    let plain = render(&sample(), Paint::PLAIN, None);
    let colored = render(
        &sample(),
        Paint {
            color: true,
            boxed: false,
            width: None,
        },
        None,
    );
    assert_ne!(plain, colored);
    assert_eq!(strip(&colored), plain);
}

#[test]
fn boxed_tables_in_a_terminal() {
    let boxed = Paint {
        color: false,
        boxed: true,
        width: None,
    };
    let text = render(&sample(), boxed, None);
    let clients = "\
╭────────┬─────────────┬────────────────┬────────────────┬──────┬──────╮
│ NAME   │ STATE       │ SINCE          │ FROM           │ OPEN │ IDLE │
├────────┼─────────────┼────────────────┼────────────────┼──────┼──────┤
│ home   │ ● connected │ 2 h 13 min     │ 81.2.3.4:51234 │    3 │    4 │
│ office │ ○ offline   │ left 5 min ago │                │    0 │    0 │
╰────────┴─────────────┴────────────────┴────────────────┴──────┴──────╯
";
    assert!(text.contains(clients), "{text}");
    assert!(
        text.contains("│ web  │ tcp   │ 0.0.0.0:443 │ 127.0.0.1:8443 │ home up, office offline │   12 │   153 │       1 │ 1.2 GB │ 8.4 GB │"),
        "{text}"
    );
    let widths: Vec<usize> = text
        .lines()
        .filter(|l| l.starts_with(['╭', '│', '├', '╰']))
        .map(|l| l.chars().count())
        .collect();
    assert_eq!(widths.iter().filter(|w| **w == 72).count(), 6, "{text}");
    let colored = render(
        &sample(),
        Paint {
            color: true,
            boxed: true,
            width: None,
        },
        None,
    );
    assert!(colored.contains("\x1b[32mhome up\x1b[0m, \x1b[31moffice offline\x1b[0m"));
    assert_eq!(strip(&colored), text);
}

fn rated() -> (Status, Rates) {
    let before = sample();
    let mut after = sample();
    after.services[0].bytes_in += 3_000_000;
    after.services[0].bytes_out += 9_000_000;
    after.services[0].total += 20;
    let r = rates(&before, &after, 1.0);
    (after, r)
}

fn fitted(width: usize, color: bool) -> String {
    let (s, r) = rated();
    let paint = Paint {
        color,
        boxed: true,
        width: Some(width),
    };
    render(&s, paint, Some(&r))
}

#[test]
fn a_wide_terminal_shows_every_column() {
    let text = fitted(400, false);
    assert!(!text.contains("hidden"), "{text}");
    assert!(!text.contains('…'), "{text}");
    let (s, r) = rated();
    let unlimited = render(
        &s,
        Paint {
            color: false,
            boxed: true,
            width: None,
        },
        Some(&r),
    );
    assert_eq!(text, unlimited);
}

#[test]
fn every_line_fits_the_terminal() {
    for width in [200, 160, 140, 120, 100, 80, 60, 40] {
        let text = fitted(width, false);
        for line in text.lines() {
            assert!(
                line.chars().count() <= width,
                "{width}: {} chars: {line}\n{text}",
                line.chars().count()
            );
        }
        assert!(text.contains("│ web "), "{width}: {text}");
        assert_eq!(strip(&fitted(width, true)), text, "{width}");
    }
}

#[test]
fn narrow_terminals_drop_the_least_useful_columns_first() {
    let header = |width: usize| {
        fitted(width, false)
            .lines()
            .find(|l| l.contains("│ NAME") && l.contains("ADDRESS"))
            .unwrap()
            .to_string()
    };
    let wide = header(400);
    for name in ["REFUSED", "TOTAL", "CONN/S", "PROTO", "IN/S", "OUT/S"] {
        assert!(wide.contains(name), "{wide}");
    }
    let medium = header(120);
    assert!(!medium.contains("REFUSED"), "{medium}");
    for name in ["ADDRESS", "CLIENTS", "OPEN", "IN/S", "OUT/S"] {
        assert!(medium.contains(name), "{medium}");
    }
    let small = header(80);
    for name in ["NAME", "ADDRESS", "CLIENTS", "OPEN", "IN/S", "OUT/S"] {
        assert!(small.contains(name), "{small}");
    }
    for name in ["REFUSED", "TOTAL", "CONN/S", "PROTO"] {
        assert!(!small.contains(name), "{small}");
    }
    let text = fitted(80, false);
    assert!(text.contains("hidden to fit: "), "{text}");
    assert!(text.contains("REFUSED"), "{text}");
}

#[test]
fn long_cells_end_with_an_ellipsis() {
    let mut s = sample();
    s.services[0].clients = (1..=6).map(|i| format!("home-{i}")).collect();
    let paint = Paint {
        color: true,
        boxed: true,
        width: Some(100),
    };
    let text = strip(&render(&s, paint, None));
    let row = text.lines().find(|l| l.starts_with("│ web")).unwrap();
    assert!(row.contains("home-1 offline, home-2…"), "{text}");
    assert!(row.chars().count() <= 100, "{row}");
}

#[test]
fn addresses_keep_their_port_when_cut() {
    let cell = cell("[2001:db8::1234:5678]:30000-30010");
    let cut = cell.clip(14, true);
    assert_eq!(cut.painted(Paint::PLAIN), "…]:30000-30010");
    assert_eq!(cut.width(), 14);
    let cut = cell.clip(14, false);
    assert_eq!(cut.painted(Paint::PLAIN), "[2001:db8::12…");
}

#[test]
fn paired_columns_are_hidden_together() {
    for width in [60, 70, 80, 90, 100, 110, 120, 130] {
        let text = fitted(width, false);
        let head = text
            .lines()
            .find(|l| l.contains("│ NAME") && l.contains("ADDRESS"))
            .unwrap();
        let has = |name: &str| head.split('│').any(|c| c.trim() == name);
        assert_eq!(has("IN"), has("OUT"), "{width}: {head}");
        assert_eq!(has("IN/S"), has("OUT/S"), "{width}: {head}");
        assert!(has("NAME") && has("ADDRESS"), "{width}: {head}");
    }
}

#[test]
fn a_long_header_goes_on_several_lines() {
    let mut s = sample();
    s.listening = vec![
        "clients on tcp 0.0.0.0:7835".into(),
        "clients on wss://tunnel.example.com:7837/burrow".into(),
        "metrics on http://127.0.0.1:9100/metrics".into(),
    ];
    let paint = Paint {
        color: false,
        boxed: true,
        width: Some(60),
    };
    let text = render(&s, paint, None);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("burrow 9.9.9  ·  up 3 d 4 h"));
    assert_eq!(lines.next(), Some("clients on tcp 0.0.0.0:7835"));
    assert_eq!(
        lines.next(),
        Some("clients on wss://tunnel.example.com:7837/burrow")
    );
}

#[test]
fn follow_adds_rates() {
    let before = sample();
    let mut after = sample();
    after.services[0].bytes_in += 3_000_000;
    after.services[0].total += 20;
    let r = rates(&before, &after, 2.0);
    assert_eq!(r["web"].in_per_sec, 1_500_000.0);
    assert_eq!(r["web"].conns_per_sec, 10.0);
    let text = render(&after, Paint::PLAIN, Some(&r));
    assert!(text.contains("CONN/S"), "{text}");
    assert!(text.contains("10.0  1.5 MB/s"), "{text}");
}

#[test]
fn websites_are_listed_with_the_tunnel_first() {
    use burrow_admin_proto::web::{HostLine, TunnelLine, WebSummary};
    let mut s = sample();
    s.web = Some(WebSummary {
        https: "[::]:443".into(),
        addresses: "IPv4 and IPv6".into(),
        shared_with_tunnel: true,
        tunnel: Some(TunnelLine {
            url: "wss://tunnel.example.com/burrow".into(),
            certificate: String::new(),
        }),
        hosts: vec![HostLine {
            name: "nas.example.com".into(),
            kind: "passthrough".into(),
            service: "nas".into(),
            ..HostLine::default()
        }],
        ..WebSummary::default()
    });
    let text = render(&s, Paint::PLAIN, None);
    let at = text.find("WEBSITES").expect("a websites table");
    let rest = &text[at..];
    let tunnel = rest.find("wss://tunnel.example.com/burrow").unwrap();
    let nas = rest.find("nas.example.com").unwrap();
    assert!(tunnel < nas, "{rest}");
    assert!(
        rest.contains("passthrough") && rest.contains("nas"),
        "{rest}"
    );
    s.web.as_mut().unwrap().hosts.clear();
    s.web.as_mut().unwrap().tunnel = None;
    let empty = render(&s, Paint::PLAIN, None);
    assert!(
        empty.contains("no names yet: every connection is closed"),
        "{empty}"
    );
}
