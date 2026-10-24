mod table;

use std::collections::HashMap;
use std::fmt::Write;

use crate::metrics::Status;
use crate::metrics::render::{bytes, duration};
use table::{ALWAYS, Cell, address, cell, col, long, num, right, table, toned};

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
    render_head(&mut out, s, paint);
    render_clients(&mut out, s, paint);
    render_services(&mut out, s, paint, rates);
    render_visitors(&mut out, s, paint);
    render_web(&mut out, s, paint);
    render_footer(&mut out, s, paint);
    out
}

fn render_head(out: &mut String, s: &Status, paint: Paint) {
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
}

fn render_clients(out: &mut String, s: &Status, paint: Paint) {
    let remote = s.clients.iter().filter(|c| !c.local).count();
    let connected = s.clients.iter().filter(|c| c.connected && !c.local).count();
    let rows = s
        .clients
        .iter()
        .map(|c| {
            let (mut state, since) = match (c.connected, c.connected_secs, c.left_secs_ago) {
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
            if c.at_limit {
                state.parts.push((", at limit".into(), Tone::Warn));
            }
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
        out,
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
}

fn render_services(out: &mut String, s: &Status, paint: Paint, rates: Option<&Rates>) {
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
        table(out, paint, &paint.bold("SERVICES"), &cols, rows);
        for n in notes {
            let note = paint.clip(&format!("  {n}"));
            let _ = writeln!(out, "{}", paint.tone(&note, Tone::Dim));
        }
    }
}

fn render_visitors(out: &mut String, s: &Status, paint: Paint) {
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
            out,
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
}

fn render_web(out: &mut String, s: &Status, paint: Paint) {
    let Some(web) = &s.web else {
        return;
    };
    let mut rows: Vec<Vec<Cell>> = web
        .tunnel
        .iter()
        .map(|t| vec![cell(&t.url), cell("tunnel"), toned("burrow", Tone::Dim)])
        .collect();
    rows.extend(
        web.hosts
            .iter()
            .map(|h| vec![cell(&h.name), cell(&h.kind), cell(&h.service)]),
    );
    if rows.is_empty() {
        let _ = writeln!(out, "\n{}", paint.dim(&web.lines().join("\n")));
        return;
    }
    table(
        out,
        paint,
        &paint.bold("WEBSITES"),
        &[
            long("NAME", ALWAYS),
            col("KIND", 5),
            long("SERVICE", ALWAYS),
        ],
        rows,
    );
}

fn render_footer(out: &mut String, s: &Status, paint: Paint) {
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
}

#[cfg(test)]
mod tests;
