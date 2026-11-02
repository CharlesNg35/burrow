use std::fmt::Write;

use super::{Paint, Tone};
use crate::metrics::Status;

pub(super) fn render_geoip(out: &mut String, s: &Status, paint: Paint, now: u64) {
    let Some(g) = &s.geoip else {
        return;
    };
    let tone = if !g.ready() || g.stale || g.failed {
        Tone::Warn
    } else {
        Tone::Dim
    };
    let _ = writeln!(
        out,
        "\n{} {}",
        paint.bold("COUNTRIES"),
        paint.tone(&g.line(now), tone)
    );
    if !g.ready() {
        let _ = writeln!(
            out,
            "{}",
            paint.tone(
                "services with allow_countries refuse public visitors until the database is ready",
                Tone::Warn
            )
        );
    }
    for line in s.services.iter().filter_map(|v| {
        burrow_syntax::country::service_line(
            &v.name,
            &v.allowed_countries,
            &v.denied_countries,
            v.refused_country,
        )
    }) {
        let _ = writeln!(out, "{}", paint.dim(&line));
    }
    if g.credit {
        let _ = writeln!(
            out,
            "{}",
            paint.dim(&format!(
                "{} ({})",
                crate::config::settings::DB_IP_CREDIT,
                crate::config::settings::DB_IP_SITE
            ))
        );
    }
}
