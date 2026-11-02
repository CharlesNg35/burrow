use burrow_admin_proto::updates::{hand_hint, platform_text, remote_text, run_text};

use super::table::{ALWAYS, cell, col, long, table, toned};
use super::{Paint, Tone, version_cell};
use crate::metrics::Status;

fn latest_line(s: &Status) -> String {
    match s.latest.as_ref() {
        Some(l) if l.version.is_some() => {
            format!(
                "latest release {}",
                l.version.as_deref().unwrap_or_default()
            )
        }
        Some(l) => match &l.error {
            Some(e) => format!("latest release not known: {e}"),
            None => "latest release not known yet".into(),
        },
        None => "latest release not checked yet".into(),
    }
}

pub fn versions(s: &Status, paint: Paint) -> String {
    let mut out = String::new();
    let rows = s
        .clients
        .iter()
        .filter(|c| !c.local)
        .map(|c| {
            let b = c.build.as_ref();
            let newer = match (&c.available, &c.blocked) {
                (Some(v), None) => toned(format!("{v} available"), Tone::Warn),
                (Some(v), Some(why)) => toned(format!("{v} available, but {why}"), Tone::Dim),
                (None, _) if b.is_none() && c.connected => toned(hand_hint(), Tone::Dim),
                _ => cell(""),
            };
            vec![
                cell(&c.name),
                version_cell(c),
                cell(b.map(platform_text).unwrap_or_default()),
                cell(b.map(run_text).unwrap_or_default()),
                cell(b.map(remote_text).unwrap_or_default()),
                newer,
            ]
        })
        .collect();
    table(
        &mut out,
        paint,
        &format!("{}  {}", paint.bold("CLIENTS"), paint.dim(&latest_line(s))),
        &[
            long("NAME", ALWAYS),
            col("VERSION", 9),
            col("SYSTEM", 5),
            col("RUNS", 4),
            long("REMOTE UPDATES", 3),
            long("NEWER", 8),
        ],
        rows,
    );
    out
}
