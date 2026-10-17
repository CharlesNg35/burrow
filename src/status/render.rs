use std::collections::HashMap;
use std::fmt::Write;

use crate::metrics::Status;
use crate::metrics::render::{bytes, duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Good,
    Bad,
    Warn,
    Accent,
    Dim,
    Bold,
}

#[derive(Debug, Clone, Copy)]
pub struct Paint {
    pub color: bool,
    pub boxed: bool,
}

impl Paint {
    pub const PLAIN: Paint = Paint {
        color: false,
        boxed: false,
    };

    fn tone(self, text: &str, tone: Tone) -> String {
        let code = match (self.color, tone) {
            (false, _) | (_, Tone::Plain) => return text.to_string(),
            (_, Tone::Good) => "32",
            (_, Tone::Bad) => "31",
            (_, Tone::Warn) => "33",
            (_, Tone::Accent) => "36",
            (_, Tone::Dim) => "2",
            (_, Tone::Bold) => "1",
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    fn bold(self, text: &str) -> String {
        self.tone(text, Tone::Bold)
    }

    fn state(self, word: &str, up: bool) -> Cell {
        let tone = if up { Tone::Good } else { Tone::Bad };
        match (self.boxed, up) {
            (false, _) => toned(word, tone),
            (true, true) => toned(format!("● {word}"), tone),
            (true, false) => toned(format!("○ {word}"), tone),
        }
    }
}

struct Cell {
    parts: Vec<(String, Tone)>,
    right: bool,
}

impl Cell {
    fn width(&self) -> usize {
        self.parts.iter().map(|(t, _)| t.chars().count()).sum()
    }

    fn painted(&self, paint: Paint) -> String {
        self.parts
            .iter()
            .map(|(t, tone)| paint.tone(t, *tone))
            .collect()
    }
}

fn toned(text: impl Into<String>, tone: Tone) -> Cell {
    Cell {
        parts: vec![(text.into(), tone)],
        right: false,
    }
}

fn cell(text: impl Into<String>) -> Cell {
    toned(text, Tone::Plain)
}

fn num(n: u64) -> Cell {
    Cell {
        right: true,
        ..toned(n.to_string(), if n == 0 { Tone::Dim } else { Tone::Plain })
    }
}

fn right(text: impl Into<String>, tone: Tone) -> Cell {
    Cell {
        right: true,
        ..toned(text, tone)
    }
}

fn table(out: &mut String, paint: Paint, title: &str, headers: &[&str], rows: Vec<Vec<Cell>>) {
    let _ = writeln!(out, "\n{title}");
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(c.width());
        }
    }
    let aligned: Vec<bool> = (0..headers.len())
        .map(|i| rows.first().is_some_and(|r| r[i].right))
        .collect();
    let pad = |text: String, width: usize, seen: usize, right_aligned: bool| {
        let fill = " ".repeat(width.saturating_sub(seen));
        if right_aligned {
            format!("{fill}{text}")
        } else {
            format!("{text}{fill}")
        }
    };
    let head: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let tone = if paint.boxed { Tone::Bold } else { Tone::Dim };
            pad(
                paint.tone(h, tone),
                widths[i],
                h.chars().count(),
                aligned[i],
            )
        })
        .collect();
    let body: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(i, c)| pad(c.painted(paint), widths[i], c.width(), c.right))
                .collect()
        })
        .collect();
    if !paint.boxed {
        for cells in std::iter::once(&head).chain(&body) {
            let line = format!("  {}", cells.join("  "));
            let _ = writeln!(out, "{}", line.trim_end());
        }
        return;
    }
    let rule = |l: &str, m: &str, r: &str| {
        let inner: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        paint.tone(&format!("{l}{}{r}", inner.join(m)), Tone::Dim)
    };
    let bar = paint.tone("│", Tone::Dim);
    let line = |cells: &Vec<String>| format!("{bar} {} {bar}", cells.join(&format!(" {bar} ")));
    let _ = writeln!(out, "{}", rule("╭", "┬", "╮"));
    let _ = writeln!(out, "{}", line(&head));
    let _ = writeln!(out, "{}", rule("├", "┼", "┤"));
    for cells in &body {
        let _ = writeln!(out, "{}", line(cells));
    }
    let _ = writeln!(out, "{}", rule("╰", "┴", "╯"));
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

fn amount(n: u64) -> Cell {
    right(bytes(n), if n == 0 { Tone::Dim } else { Tone::Plain })
}

fn speed(bytes_per_sec: f64) -> Cell {
    if bytes_per_sec < 1.0 {
        return right("-", Tone::Dim);
    }
    right(format!("{}/s", bytes(bytes_per_sec as u64)), Tone::Accent)
}

pub fn render(s: &Status, paint: Paint, rates: Option<&Rates>) -> String {
    let mut out = String::new();
    let mut head = vec![
        paint.bold(&format!("burrow {}", s.version)),
        format!("up {}", duration(s.uptime_secs)),
    ];
    head.extend(s.listening.iter().cloned());
    let _ = writeln!(out, "{}", head.join(&paint.tone("  ·  ", Tone::Dim)));

    let connected = s.clients.iter().filter(|c| c.connected).count();
    let rows = s
        .clients
        .iter()
        .map(|c| {
            let (state, since) = match (c.connected, c.connected_secs, c.left_secs_ago) {
                (true, secs, _) => (
                    paint.state("connected", true),
                    cell(secs.map(duration).unwrap_or_default()),
                ),
                (false, _, Some(ago)) => (
                    paint.state("offline", false),
                    toned(format!("left {} ago", duration(ago)), Tone::Dim),
                ),
                (false, _, None) => (
                    paint.state("offline", false),
                    toned("never seen", Tone::Dim),
                ),
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
    let all_in = connected == s.clients.len();
    table(
        &mut out,
        paint,
        &format!(
            "{}  {}",
            paint.bold("CLIENTS"),
            paint.tone(
                &format!("{connected}/{} connected", s.clients.len()),
                if all_in { Tone::Good } else { Tone::Warn }
            )
        ),
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
                let mut clients = Cell {
                    parts: Vec::new(),
                    right: false,
                };
                for (i, c) in v.clients.iter().enumerate() {
                    if i > 0 {
                        clients.parts.push((", ".into(), Tone::Plain));
                    }
                    let health = v.health.iter().find(|h| &h.client == c);
                    clients.parts.push(match (v.health.is_empty(), health) {
                        (true, _) => (c.clone(), Tone::Plain),
                        (false, Some(h)) if h.up => (format!("{c} up"), Tone::Good),
                        (false, Some(_)) => (format!("{c} down"), Tone::Bad),
                        (false, None) => (format!("{c} offline"), Tone::Bad),
                    });
                }
                let mut row = vec![
                    cell(&v.name),
                    cell(&v.protocol),
                    match &v.bind {
                        Some(b) => cell(b),
                        None => toned("private", Tone::Dim),
                    },
                    cell(&v.target),
                    clients,
                    num(v.open),
                    num(v.total),
                    right(
                        v.refused.to_string(),
                        if v.refused > 0 { Tone::Warn } else { Tone::Dim },
                    ),
                    amount(v.bytes_in),
                    amount(v.bytes_out),
                ];
                if let Some(rates) = rates {
                    let r = rates.get(&v.name).copied().unwrap_or_default();
                    row.push(if r.conns_per_sec < 0.05 {
                        right("-", Tone::Dim)
                    } else {
                        right(format!("{:.1}", r.conns_per_sec), Tone::Accent)
                    });
                    row.push(speed(r.in_per_sec));
                    row.push(speed(r.out_per_sec));
                }
                let mut extra = Vec::new();
                if let Some(m) = &v.mode {
                    extra.push(m.clone());
                }
                if let Some(b) = v.bandwidth {
                    extra.push(format!("limit {}/s", bytes(b)));
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
        table(&mut out, paint, &paint.bold("SERVICES"), &headers, rows);
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
            &paint.bold("VISITORS"),
            &["NAME", "OPEN", "TOTAL"],
            rows,
        );
    }

    let mut footer = Vec::new();
    match s.udp_peers {
        0 => {}
        1 => footer.push("1 UDP peer".into()),
        n => footer.push(format!("{n} UDP peers")),
    }
    if s.handshakes_refused > 0 {
        footer.push(format!(
            "{} refused handshakes since start (wrong secret, unknown name or limits)",
            s.handshakes_refused
        ));
    }
    if !footer.is_empty() {
        let _ = writeln!(out, "\n{}", paint.tone(&footer.join("  ·  "), Tone::Dim));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{ClientStatus, HealthStatus, ServiceStatus, VisitorStatus};

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
                    }],
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
  NAME    OPEN  TOTAL
  laptop     1     12

2 refused handshakes since start (wrong secret, unknown name or limits)
"
        );
    }

    #[test]
    fn colors_keep_the_columns_aligned() {
        let plain = render(&sample(), Paint::PLAIN, None);
        let colored = render(
            &sample(),
            Paint {
                color: true,
                boxed: false,
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
            },
            None,
        );
        assert!(colored.contains("\x1b[32mhome up\x1b[0m, \x1b[31moffice offline\x1b[0m"));
        assert_eq!(strip(&colored), text);
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
}
