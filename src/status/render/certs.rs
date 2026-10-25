use std::fmt::Write;

use super::table::{ALWAYS, Cell, cell, col, long, table, toned};
use super::{Paint, Tone};
use crate::metrics::render::duration;
use crate::metrics::{CertSourceStatus, CertStateStatus, CertStatus, Status};
use burrow_admin_proto::status::CertCheck;

pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn source(c: &CertStatus) -> String {
    let base = match &c.source {
        CertSourceStatus::OwnFile { path } => format!("own file {path}"),
        CertSourceStatus::SharedFile { path } => format!("web.certificates file {path}"),
        CertSourceStatus::TunnelFile { path } => format!("websocket.tls {path}"),
        CertSourceStatus::LetsEncrypt { .. } => "Let's Encrypt".into(),
        CertSourceStatus::Passthrough | CertSourceStatus::Plain => String::new(),
    };
    match c.check {
        Some(CertCheck::Tls) => format!("{base}, TLS check"),
        Some(CertCheck::Http) => format!("{base}, HTTP check"),
        None => base,
    }
}

fn state(c: &CertStatus) -> Cell {
    match &c.state {
        CertStateStatus::Ok => toned("ok", Tone::Good),
        CertStateStatus::Pending => toned("pending", Tone::Warn),
        CertStateStatus::Renewing => toned("renewing", Tone::Accent),
        CertStateStatus::Failing { .. } => toned("failing", Tone::Bad),
    }
}

fn when(at: Option<u64>, now: u64) -> Cell {
    match at {
        None => toned("-", Tone::Dim),
        Some(t) if t >= now && t - now >= 365 * 86_400 => cell(burrow_admin_proto::date::date(t)),
        Some(t) if t >= now => cell(format!("in {}", duration(t - now))),
        Some(t) => toned(format!("expired {} ago", duration(now - t)), Tone::Bad),
    }
}

fn next(at: Option<u64>, now: u64) -> Cell {
    match at {
        None => toned("-", Tone::Dim),
        Some(t) => cell(format!("in {}", duration(t.saturating_sub(now)))),
    }
}

fn burrows(c: &&CertStatus) -> bool {
    !matches!(
        c.source,
        CertSourceStatus::Passthrough | CertSourceStatus::Plain
    )
}

pub(super) fn render_certs(out: &mut String, s: &Status, paint: Paint, now: u64) {
    let mine: Vec<&CertStatus> = s.certificates.iter().filter(burrows).collect();
    if mine.is_empty() {
        return;
    }
    let rows = mine
        .iter()
        .map(|c| {
            vec![
                cell(&c.name),
                cell(source(c)),
                state(c),
                when(c.expires, now),
                next(c.next_renewal, now),
            ]
        })
        .collect();
    table(
        out,
        paint,
        &paint.bold("CERTIFICATES"),
        &[
            long("NAME", ALWAYS),
            long("SOURCE", 3),
            col("STATE", ALWAYS),
            col("EXPIRES", 4),
            col("NEXT RENEWAL", 5),
        ],
        rows,
    );
    for c in &mine {
        let CertStateStatus::Failing { reason } = &c.state else {
            continue;
        };
        let mut line = format!("{}: {reason}", c.name);
        if c.failures > 1 {
            line.push_str(&format!(" ({} failures)", c.failures));
        }
        if let Some(url) = &c.explanation {
            line.push_str(&format!("; see {url}"));
        }
        let _ = writeln!(out, "{}", paint.tone(&paint.clip(&line), Tone::Bad));
    }
    let queue = s.acme.as_ref().map_or(&[][..], |a| a.queue.as_slice());
    if !queue.is_empty() {
        let _ = writeln!(out, "Certificate orders, one at a time:");
        for o in queue {
            let line = format!("  {}", burrow_admin_proto::date::order_line(o, now));
            let _ = writeln!(out, "{}", paint.clip(&line));
        }
    }
    if let Some(acme) = &s.acme {
        let mut line = match &acme.account {
            Some(a) => format!("Let's Encrypt account {a}"),
            None => "Let's Encrypt account not created yet".to_string(),
        };
        if !acme.state_dir.is_empty() {
            line.push_str(&format!(", kept in {}", acme.state_dir));
        }
        let _ = writeln!(out, "{}", paint.dim(&paint.clip(&line)));
    }
}
