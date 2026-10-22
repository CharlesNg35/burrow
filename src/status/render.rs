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
    pub width: Option<usize>,
}

impl Paint {
    pub const PLAIN: Paint = Paint {
        color: false,
        boxed: false,
        width: None,
    };

    pub fn clip(self, text: &str) -> String {
        match self.width {
            Some(w) if text.chars().count() > w => {
                let cut: String = text.chars().take(w.saturating_sub(1)).collect();
                let mut cut = cut.trim_end().to_string();
                cut.push('…');
                cut
            }
            _ => text.to_string(),
        }
    }

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

    pub fn dim(self, text: &str) -> String {
        self.tone(text, Tone::Dim)
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

#[derive(Clone)]
struct Cell {
    parts: Vec<(String, Tone)>,
    right: bool,
}

impl Cell {
    fn clip(&self, width: usize, tail: bool) -> Cell {
        if self.width() <= width {
            return self.clone();
        }
        if tail {
            return self.clip_start(width);
        }
        let mut left = width.saturating_sub(1);
        let mut parts = Vec::new();
        let mut last = Tone::Plain;
        for (text, tone) in &self.parts {
            if left == 0 {
                break;
            }
            let taken: String = text.chars().take(left).collect();
            left -= taken.chars().count();
            last = *tone;
            parts.push((taken, *tone));
        }
        while let Some((text, _)) = parts.last_mut() {
            let trimmed = text.trim_end().len();
            text.truncate(trimmed);
            if !text.is_empty() {
                break;
            }
            parts.pop();
        }
        parts.push(("…".into(), last));
        Cell {
            parts,
            right: self.right,
        }
    }

    fn clip_start(&self, width: usize) -> Cell {
        let mut left = width.saturating_sub(1);
        let mut parts = Vec::new();
        let mut first = Tone::Plain;
        for (text, tone) in self.parts.iter().rev() {
            if left == 0 {
                break;
            }
            let count = text.chars().count();
            let taken: String = text.chars().skip(count.saturating_sub(left)).collect();
            left -= taken.chars().count();
            first = *tone;
            parts.push((taken, *tone));
        }
        parts.reverse();
        while let Some((text, _)) = parts.first_mut() {
            *text = text.trim_start().to_string();
            if !text.is_empty() {
                break;
            }
            parts.remove(0);
        }
        parts.insert(0, ("…".into(), first));
        Cell {
            parts,
            right: self.right,
        }
    }

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

const ALWAYS: u8 = u8::MAX;
const CAP: usize = 24;
const FLOOR: usize = 10;
const CHEAP: u8 = 3;

struct Col {
    name: &'static str,
    keep: u8,
    long: bool,
    tail: bool,
}

fn col(name: &'static str, keep: u8) -> Col {
    Col {
        name,
        keep,
        long: false,
        tail: false,
    }
}

fn long(name: &'static str, keep: u8) -> Col {
    Col {
        long: true,
        ..col(name, keep)
    }
}

fn address(name: &'static str, keep: u8) -> Col {
    Col {
        tail: true,
        ..long(name, keep)
    }
}

fn span(widths: &[usize], shown: &[bool], boxed: bool) -> usize {
    let visible: Vec<usize> = widths
        .iter()
        .zip(shown)
        .filter(|(_, s)| **s)
        .map(|(w, _)| *w)
        .collect();
    if boxed {
        1 + visible.iter().map(|w| w + 3).sum::<usize>()
    } else {
        2 + visible.iter().sum::<usize>() + 2 * visible.len().saturating_sub(1)
    }
}

fn fit(cols: &[Col], widths: &mut [usize], shown: &mut [bool], max: usize, boxed: bool) {
    if span(widths, shown, boxed) <= max {
        return;
    }
    let floor = |i: usize| FLOOR.max(cols[i].name.chars().count());
    for (i, c) in cols.iter().enumerate() {
        if c.long {
            widths[i] = widths[i].min(CAP.max(floor(i)));
        }
    }
    loop {
        let need = span(widths, shown, boxed).saturating_sub(max);
        if need == 0 {
            return;
        }
        let room: usize = (0..cols.len())
            .filter(|&i| shown[i] && cols[i].long && !cols[i].tail)
            .map(|i| widths[i].saturating_sub(floor(i)))
            .sum();
        let weakest = (0..cols.len())
            .filter(|&i| shown[i] && cols[i].keep < ALWAYS)
            .min_by_key(|&i| (cols[i].keep, std::cmp::Reverse(i)));
        match weakest {
            Some(i) if cols[i].keep <= CHEAP || room < need => {
                let keep = cols[i].keep;
                for (j, c) in cols.iter().enumerate() {
                    if c.keep == keep {
                        shown[j] = false;
                    }
                }
            }
            _ => {
                let any = weakest.is_none();
                for _ in 0..need {
                    let widest = (0..cols.len())
                        .filter(|&i| {
                            shown[i]
                                && cols[i].long
                                && (any || !cols[i].tail)
                                && widths[i] > floor(i)
                        })
                        .max_by_key(|&i| widths[i] - floor(i));
                    match widest {
                        Some(i) => widths[i] -= 1,
                        None => return,
                    }
                }
                return;
            }
        }
    }
}

fn table(out: &mut String, paint: Paint, title: &str, cols: &[Col], rows: Vec<Vec<Cell>>) {
    let _ = writeln!(out, "\n{title}");
    let mut widths: Vec<usize> = cols.iter().map(|c| c.name.chars().count()).collect();
    for row in &rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(c.width());
        }
    }
    let mut shown = vec![true; cols.len()];
    if let Some(max) = paint.width {
        fit(cols, &mut widths, &mut shown, max, paint.boxed);
    }
    let visible: Vec<usize> = (0..cols.len()).filter(|&i| shown[i]).collect();
    let aligned: Vec<bool> = (0..cols.len())
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
    let head: Vec<String> = visible
        .iter()
        .map(|&i| {
            let tone = if paint.boxed { Tone::Bold } else { Tone::Dim };
            let name = cols[i].name;
            pad(
                paint.tone(name, tone),
                widths[i],
                name.chars().count(),
                aligned[i],
            )
        })
        .collect();
    let body: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            visible
                .iter()
                .map(|&i| {
                    let c = row[i].clip(widths[i], cols[i].tail);
                    pad(c.painted(paint), widths[i], c.width(), c.right)
                })
                .collect()
        })
        .collect();
    if paint.boxed {
        let rule = |l: &str, m: &str, r: &str| {
            let inner: Vec<String> = visible.iter().map(|&i| "─".repeat(widths[i] + 2)).collect();
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
    } else {
        for cells in std::iter::once(&head).chain(&body) {
            let line = format!("  {}", cells.join("  "));
            let _ = writeln!(out, "{}", line.trim_end());
        }
    }
    let hidden: Vec<&str> = (0..cols.len())
        .filter(|&i| !shown[i])
        .map(|i| cols[i].name)
        .collect();
    if !hidden.is_empty() {
        let note = paint.clip(&format!("  hidden to fit: {}", hidden.join(", ")));
        let _ = writeln!(out, "{}", paint.tone(&note, Tone::Dim));
    }
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
    let title = format!("burrow {}", s.version);
    let up = format!("up {}", duration(s.uptime_secs));
    let dot = paint.tone("  ·  ", Tone::Dim);
    let one_line = [title.clone(), up.clone()]
        .iter()
        .chain(&s.listening)
        .map(|p| p.chars().count())
        .sum::<usize>()
        + 5 * s.listening.len()
        + 5;
    if paint.width.is_none_or(|w| one_line <= w) {
        let mut head = vec![paint.bold(&title), up];
        head.extend(s.listening.iter().cloned());
        let _ = writeln!(out, "{}", head.join(&dot));
    } else {
        let _ = writeln!(
            out,
            "{}",
            paint.bold(&paint.clip(&format!("{title}  ·  {up}")))
        );
        for l in &s.listening {
            let _ = writeln!(out, "{}", paint.clip(l));
        }
    }

    let remote = s.clients.iter().filter(|c| !c.local).count();
    let connected = s.clients.iter().filter(|c| c.connected && !c.local).count();
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
                match c.local {
                    true => toned("this server", Tone::Dim),
                    false => cell(c.from.clone().unwrap_or_default()),
                },
                num(c.open),
                num(c.idle_channels),
            ]
        })
        .collect();
    let all_in = connected == remote;
    table(
        &mut out,
        paint,
        &format!(
            "{}  {}",
            paint.bold("CLIENTS"),
            paint.tone(
                &format!("{connected}/{remote} connected"),
                if all_in { Tone::Good } else { Tone::Warn }
            )
        ),
        &[
            long("NAME", ALWAYS),
            col("STATE", 9),
            col("SINCE", 5),
            long("FROM", 4),
            col("OPEN", 7),
            col("IDLE", 1),
        ],
        rows,
    );

    if !s.services.is_empty() {
        let totals = if rates.is_some() { 4 } else { 6 };
        let mut cols = vec![
            long("NAME", ALWAYS),
            col("PROTO", 3),
            address("ADDRESS", 11),
            address("TARGET", 5),
            long("CLIENTS", 9),
            col("OPEN", 8),
            col("TOTAL", 2),
            col("REFUSED", 1),
            col("IN", totals),
            col("OUT", totals),
        ];
        if rates.is_some() {
            cols.extend([col("CONN/S", 2), col("IN/S", 10), col("OUT/S", 10)]);
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
                    let aside = v.set_aside.iter().find(|a| &a.client == c);
                    clients
                        .parts
                        .push(match (v.health.is_empty(), health, aside) {
                            (_, _, Some(a)) if a.refused => (format!("{c} refused"), Tone::Bad),
                            (false, Some(h), _) if !h.up => (format!("{c} down"), Tone::Bad),
                            (_, _, Some(_)) => (format!("{c} failing"), Tone::Warn),
                            (true, _, None) => (c.clone(), Tone::Plain),
                            (false, Some(_), None) => (format!("{c} up"), Tone::Good),
                            (false, None, None) => (format!("{c} offline"), Tone::Bad),
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
        table(&mut out, paint, &paint.bold("SERVICES"), &cols, rows);
        for n in notes {
            let note = paint.clip(&format!("  {n}"));
            let _ = writeln!(out, "{}", paint.tone(&note, Tone::Dim));
        }
    }

    if !s.visitors.is_empty() {
        let rows = s
            .visitors
            .iter()
            .map(|v| {
                let last = match (v.open, v.last_seen_secs) {
                    (1.., _) => cell("now"),
                    (0, Some(ago)) => cell(format!("{} ago", duration(ago))),
                    (0, None) => toned("not since start", Tone::Dim),
                };
                vec![cell(&v.name), last, num(v.open), num(v.total)]
            })
            .collect();
        table(
            &mut out,
            paint,
            &paint.bold("VISITORS"),
            &[
                long("NAME", ALWAYS),
                col("LAST USED", 7),
                col("OPEN", 6),
                col("TOTAL", 2),
            ],
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
        let line = paint.clip(&footer.join("  ·  "));
        let _ = writeln!(out, "\n{}", paint.tone(&line, Tone::Dim));
    }
    out
}

#[cfg(test)]
mod tests {
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
                    set_aside: vec![],
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
        }];
        let text = render(&s, Paint::PLAIN, None);
        assert!(text.contains("home down, office offline"), "{text}");
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
}
