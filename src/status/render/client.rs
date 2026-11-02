use std::fmt::Write;

use burrow_admin_proto::updates::LocalClient;
use burrow_admin_proto::updates::{job_text, platform_text, remote_text, run_text};

use super::{Paint, Tone};
use burrow_admin_proto::status::ONLINE;
use burrow_syntax::duration::span;

pub(super) fn render_local(out: &mut String, c: &LocalClient, version: &str, paint: Paint) {
    let state = match c.connected_secs {
        Some(secs) => paint.tone(&format!("{ONLINE} for {}", span(secs)), Tone::Good),
        None => paint.tone("connecting", Tone::Warn),
    };
    let _ = writeln!(
        out,
        "{}",
        paint.clip(&format!(
            "{}  ·  client \"{}\" of {}  ·  {state}",
            paint.bold(&format!("burrow {version}")),
            c.name,
            c.server
        ))
    );
    let b = &c.build;
    let rows = [
        ("runs", format!("{} on {}", run_text(b), platform_text(b))),
        ("remote updates", remote_text(b)),
    ];
    for (label, value) in rows {
        let _ = writeln!(out, "{}", paint.clip(&format!("  {label:<15} {value}")));
    }
    if let Some(u) = &c.last_update {
        let line = format!("  {:<15} {}", "last update", job_text(u));
        let tone = if u.state.failed() {
            Tone::Warn
        } else {
            Tone::Plain
        };
        let _ = writeln!(out, "{}", paint.tone(&paint.clip(&line), tone));
    }
}
