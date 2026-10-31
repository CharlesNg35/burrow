use super::table::{ALWAYS, cell, col, long, num, table, toned};
use super::{Paint, Tone};
use crate::metrics::Status;
use crate::metrics::render::duration;

pub(super) fn render_notify(out: &mut String, s: &Status, paint: Paint) {
    if s.notify.is_empty() {
        return;
    }
    let rows = s
        .notify
        .iter()
        .map(|n| {
            let state = match (&n.failing, n.failing_secs) {
                (Some(why), Some(secs)) => {
                    toned(format!("failing for {}: {why}", duration(secs)), Tone::Bad)
                }
                (Some(why), None) => toned(format!("failing: {why}"), Tone::Bad),
                (None, _) => toned("ok", Tone::Good),
            };
            vec![
                cell(&n.name),
                num(n.sent),
                num(n.waiting),
                num(n.dropped),
                state,
            ]
        })
        .collect();
    table(
        out,
        paint,
        &paint.bold("NOTIFICATIONS"),
        &[
            long("TO", ALWAYS),
            col("SENT", 4),
            col("WAITING", 3),
            col("LOST", 2),
            long("STATE", ALWAYS),
        ],
        rows,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::status::NotifyStatus;

    #[test]
    fn each_destination_shows_its_counts_and_a_failure_with_its_reason() {
        let s = Status {
            notify: vec![
                NotifyStatus {
                    name: "ntfy ntfy.sh".into(),
                    sent: 3,
                    ..NotifyStatus::default()
                },
                NotifyStatus {
                    name: "email ops@example.com".into(),
                    waiting: 1,
                    failing: Some("connection refused".into()),
                    failing_secs: Some(120),
                    ..NotifyStatus::default()
                },
            ],
            ..Status::default()
        };
        let mut out = String::new();
        render_notify(&mut out, &s, Paint::PLAIN);
        assert!(out.contains("NOTIFICATIONS"), "{out}");
        assert!(out.contains("ntfy ntfy.sh"), "{out}");
        assert!(
            out.contains("failing for 2 min: connection refused"),
            "{out}"
        );
        let mut none = String::new();
        render_notify(&mut none, &Status::default(), Paint::PLAIN);
        assert!(none.is_empty());
    }
}
