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
    for svc in s.services.iter().filter(|v| v.countries.is_some()) {
        let rule = svc.countries.as_deref().unwrap_or_default();
        let _ = writeln!(
            out,
            "{}",
            paint.dim(&format!(
                "{}: {rule}; {} refused by country",
                svc.name, svc.refused_country
            ))
        );
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
