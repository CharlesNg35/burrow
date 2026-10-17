use std::collections::HashMap;
use std::fmt::Write;

use crate::metrics::Status;
use crate::metrics::render::{bytes, duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Good,
    Bad,
    Dim,
}

#[derive(Debug, Clone, Copy)]
pub struct Paint {
    pub color: bool,
}

impl Paint {
    fn tone(self, text: &str, tone: Tone) -> String {
        let code = match (self.color, tone) {
            (false, _) | (_, Tone::Plain) => return text.to_string(),
            (_, Tone::Good) => "32",
            (_, Tone::Bad) => "31",
            (_, Tone::Dim) => "2",
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    fn bold(self, text: &str) -> String {
        if self.color {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

struct Cell {
    text: String,
    tone: Tone,
    right: bool,
}

fn cell(text: impl Into<String>) -> Cell {
    Cell {
        text: text.into(),
        tone: Tone::Plain,
        right: false,
    }
}

fn num(n: u64) -> Cell {
    Cell {
        text: n.to_string(),
        tone: if n == 0 { Tone::Dim } else { Tone::Plain },
        right: true,
    }
}

fn right(text: impl Into<String>) -> Cell {
    Cell {
        right: true,
        ..cell(text)
    }
}

fn toned(text: impl Into<String>, tone: Tone) -> Cell {
    Cell { tone, ..cell(text) }
}

fn table(out: &mut String, paint: Paint, title: &str, headers: &[&str], rows: Vec<Vec<Cell>>) {
    let _ = writeln!(out, "\n{}", paint.bold(title));
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(c.text.chars().count());
        }
    }
    let aligned: Vec<bool> = (0..headers.len())
        .map(|i| rows.first().is_some_and(|r| r[i].right))
        .collect();
    let line = |cells: Vec<(String, usize, bool)>| {
        let mut s = String::from(" ");
        for (text, width, right_aligned) in cells {
            let pad = width.saturating_sub(strip(&text).chars().count());
            s.push(' ');
            if right_aligned {
                s.push_str(&" ".repeat(pad));
                s.push_str(&text);
            } else {
                s.push_str(&text);
                s.push_str(&" ".repeat(pad));
            }
            s.push(' ');
        }
        s.trim_end().to_string()
    };
    let head = headers
        .iter()
        .enumerate()
        .map(|(i, h)| (paint.tone(h, Tone::Dim), widths[i], aligned[i]))
        .collect();
    let _ = writeln!(out, "{}", line(head));
    for row in rows {
        let cells = row
            .into_iter()
            .enumerate()
            .map(|(i, c)| (paint.tone(&c.text, c.tone), widths[i], c.right))
            .collect();
        let _ = writeln!(out, "{}", line(cells));
    }
}

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

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rate {
    pub in_per_sec: f64,
    pub out_per_sec: f64,
    pub conns_per_sec: f64,
}

pub type Rates = HashMap<String, Rate>;

pub fn rates(before: &Status, after: &Status, secs: f64) -> Rates {
    let secs = secs.max(0.001);
    after
        .services
        .iter()
        .filter_map(|s| {
            let b = before.services.iter().find(|b| b.name == s.name)?;
            let per = |now: u64, then: u64| now.saturating_sub(then) as f64 / secs;
            Some((
                s.name.clone(),
                Rate {
                    in_per_sec: per(s.bytes_in, b.bytes_in),
                    out_per_sec: per(s.bytes_out, b.bytes_out),
                    conns_per_sec: per(s.total, b.total),
                },
            ))
        })
        .collect()
}

fn speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec < 1.0 {
        return "-".into();
    }
    format!("{}/s", bytes(bytes_per_sec as u64))
}

pub fn render(s: &Status, paint: Paint, rates: Option<&Rates>) -> String {
    let mut out = String::new();
    let mut head = vec![
        paint.bold(&format!("burrow {}", s.version)),
        format!("up {}", duration(s.uptime_secs)),
    ];
    head.extend(s.listening.iter().cloned());
    let _ = writeln!(out, "{}", head.join("  ·  "));

    let connected = s.clients.iter().filter(|c| c.connected).count();
    let rows = s
        .clients
        .iter()
        .map(|c| {
            let (state, since) = match (c.connected, c.connected_secs, c.left_secs_ago) {
                (true, secs, _) => (
                    toned("connected", Tone::Good),
                    cell(secs.map(duration).unwrap_or_default()),
                ),
                (false, _, Some(ago)) => (
                    toned("offline", Tone::Bad),
                    toned(format!("left {} ago", duration(ago)), Tone::Dim),
                ),
                (false, _, None) => (toned("offline", Tone::Bad), toned("never seen", Tone::Dim)),
            };
            vec![
                cell(&c.name),
                state,
                since,
                cell(c.from.clone().unwrap_or_default()),
                num(c.open),
                num(c.idle_channels),
            ]
        })
        .collect();
    table(
        &mut out,
        paint,
        &format!("CLIENTS  {connected}/{} connected", s.clients.len()),
        &["NAME", "STATE", "SINCE", "FROM", "OPEN", "IDLE"],
        rows,
    );

    if !s.services.is_empty() {
        let mut headers = vec![
            "NAME", "PROTO", "ADDRESS", "TARGET", "CLIENTS", "OPEN", "TOTAL", "REFUSED", "IN",
            "OUT",
        ];
        if rates.is_some() {
            headers.extend(["CONN/S", "IN/S", "OUT/S"]);
        }
        let mut notes = Vec::new();
        let rows = s
            .services
            .iter()
            .map(|v| {
                let clients = if v.health.is_empty() {
                    v.clients.join(", ")
                } else {
                    v.clients
                        .iter()
                        .map(|c| match v.health.iter().find(|h| &h.client == c) {
                            Some(h) if h.up => format!("{c} up"),
                            Some(_) => format!("{c} down"),
                            None => format!("{c} offline"),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let down = v.health.iter().any(|h| !h.up);
                let mut row = vec![
                    cell(&v.name),
                    cell(&v.protocol),
                    match &v.bind {
                        Some(b) => cell(b),
                        None => toned("private", Tone::Dim),
                    },
                    cell(&v.target),
                    toned(clients, if down { Tone::Bad } else { Tone::Plain }),
                    num(v.open),
                    num(v.total),
                    Cell {
                        tone: if v.refused > 0 { Tone::Bad } else { Tone::Dim },
                        ..num(v.refused)
                    },
                    right(bytes(v.bytes_in)),
                    right(bytes(v.bytes_out)),
                ];
                if let Some(rates) = rates {
                    let r = rates.get(&v.name).copied().unwrap_or_default();
                    row.push(right(if r.conns_per_sec < 0.05 {
                        "-".to_string()
                    } else {
                        format!("{:.1}", r.conns_per_sec)
                    }));
                    row.push(right(speed(r.in_per_sec)));
                    row.push(right(speed(r.out_per_sec)));
                }
                let mut extra = Vec::new();
                if let Some(m) = &v.mode {
                    extra.push(m.clone());
                }
                if let Some(l) = &v.limit {
                    extra.push(format!("limit {l}"));
                }
                if let Some(p) = &v.proxy_protocol {
                    extra.push(format!("proxy protocol {p}"));
                }
                if v.filtered {
                    extra.push("source filter".into());
                }
                if !v.visitors.is_empty() {
                    extra.push(format!("visitors {}", v.visitors.join(", ")));
                }
                if !extra.is_empty() {
                    notes.push(format!("{}: {}", v.name, extra.join(", ")));
                }
                row
            })
            .collect();
        table(&mut out, paint, "SERVICES", &headers, rows);
        for n in notes {
            let _ = writeln!(out, "  {}", paint.tone(&n, Tone::Dim));
        }
    }

    if !s.visitors.is_empty() {
        let rows = s
            .visitors
            .iter()
            .map(|v| vec![cell(&v.name), num(v.open), num(v.total)])
            .collect();
        table(
            &mut out,
            paint,
            "VISITORS",
            &["NAME", "OPEN", "TOTAL"],
            rows,
        );
    }

    let mut footer = Vec::new();
    if s.udp_peers > 0 {
        footer.push(format!("{} UDP peers", s.udp_peers));
    }
    if s.handshakes_refused > 0 {
        footer.push(format!(
            "{} refused handshakes since start (wrong secret, unknown name or limits)",
            s.handshakes_refused
        ));
    }
    if !footer.is_empty() {
        let _ = writeln!(out, "\n{}", footer.join("  ·  "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{ClientStatus, HealthStatus, ServiceStatus, VisitorStatus};

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
                },
                ClientStatus {
                    name: "office".into(),
                    connected: false,
                    connected_secs: None,
                    from: None,
                    open: 0,
                    idle_channels: 0,
                    left_secs_ago: Some(300),
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
                    limit: Some("40mbit".into()),
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
                    }],
                },
                ServiceStatus {
                    name: "db".into(),
                    protocol: "tcp".into(),
                    bind: None,
                    target: "127.0.0.1:5432".into(),
                    clients: vec!["home".into()],
                    mode: None,
                    limit: None,
                    visitors: vec!["laptop".into()],
                    proxy_protocol: None,
                    filtered: false,
                    open: 0,
                    total: 0,
                    refused: 0,
                    bytes_in: 0,
                    bytes_out: 0,
                    health: vec![],
                },
            ],
            visitors: vec![VisitorStatus {
                name: "laptop".into(),
                open: 1,
                total: 12,
            }],
        }
    }

    #[test]
    fn plain_tables() {
        let text = render(&sample(), Paint { color: false }, None);
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
  web: balance, limit 40mbit, proxy protocol v2, source filter
  db: visitors laptop

VISITORS
  NAME    OPEN  TOTAL
  laptop     1     12

2 refused handshakes since start (wrong secret, unknown name or limits)
"
        );
    }

    #[test]
    fn colors_keep_the_columns_aligned() {
        let plain = render(&sample(), Paint { color: false }, None);
        let colored = render(&sample(), Paint { color: true }, None);
        assert_ne!(plain, colored);
        assert_eq!(strip(&colored), plain);
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
        let text = render(&after, Paint { color: false }, Some(&r));
        assert!(text.contains("CONN/S"), "{text}");
        assert!(text.contains("10.0  1.5 MB/s"), "{text}");
    }
}
