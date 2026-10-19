use burrow_admin_proto::status::Status;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rate {
    pub bytes_in: f64,
    pub bytes_out: f64,
    pub connections: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rates {
    pub services: Vec<(String, Rate)>,
    pub total: Rate,
}

impl Rates {
    pub fn service(&self, name: &str) -> Rate {
        self.services
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, r)| *r)
            .unwrap_or_default()
    }
}

fn per_sec(new: u64, old: u64, secs: f64) -> f64 {
    if new < old || secs <= 0.0 {
        0.0
    } else {
        (new - old) as f64 / secs
    }
}

pub fn rates(old: &Status, new: &Status, secs: f64) -> Rates {
    let restarted = new.uptime_secs < old.uptime_secs;
    let mut out = Rates::default();
    for s in &new.services {
        let r = match old.services.iter().find(|o| o.name == s.name) {
            Some(o) if !restarted => Rate {
                bytes_in: per_sec(s.bytes_in, o.bytes_in, secs),
                bytes_out: per_sec(s.bytes_out, o.bytes_out, secs),
                connections: per_sec(s.total, o.total, secs),
            },
            _ => Rate::default(),
        };
        out.total.bytes_in += r.bytes_in;
        out.total.bytes_out += r.bytes_out;
        out.total.connections += r.connections;
        out.services.push((s.name.clone(), r));
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct History {
    cap: usize,
    points: std::collections::VecDeque<f64>,
}

impl History {
    pub fn new(cap: usize) -> Self {
        History {
            cap: cap.max(1),
            points: std::collections::VecDeque::with_capacity(cap.max(1)),
        }
    }

    pub fn push(&mut self, v: f64) {
        if self.points.len() == self.cap {
            self.points.pop_front();
        }
        self.points
            .push_back(if v.is_finite() { v.max(0.0) } else { 0.0 });
    }

    pub fn points(&self) -> impl Iterator<Item = f64> + '_ {
        self.points.iter().copied()
    }

    pub fn max(&self) -> f64 {
        self.points.iter().copied().fold(0.0, f64::max)
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

pub fn human_bytes(bytes_per_sec: f64) -> String {
    let units = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut v = bytes_per_sec.max(0.0);
    let mut i = 0;
    while v >= 1000.0 && i + 1 < units.len() {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 || v >= 100.0 {
        format!("{v:.0} {}", units[i])
    } else {
        format!("{v:.1} {}", units[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::status::ServiceStatus;

    fn status(uptime: u64, svc: &[(&str, u64, u64, u64)]) -> Status {
        Status {
            uptime_secs: uptime,
            services: svc
                .iter()
                .map(|(n, i, o, t)| ServiceStatus {
                    name: n.to_string(),
                    bytes_in: *i,
                    bytes_out: *o,
                    total: *t,
                    ..ServiceStatus::default()
                })
                .collect(),
            ..Status::default()
        }
    }

    #[test]
    fn rates_come_from_counter_deltas() {
        let a = status(10, &[("web", 1000, 5000, 3), ("ssh", 0, 0, 0)]);
        let b = status(12, &[("web", 3000, 9000, 7), ("new", 50, 50, 1)]);
        let r = rates(&a, &b, 2.0);
        assert_eq!(
            r.service("web"),
            Rate {
                bytes_in: 1000.0,
                bytes_out: 2000.0,
                connections: 2.0
            }
        );
        assert_eq!(r.service("new"), Rate::default(), "no earlier sample");
        assert_eq!(r.total.bytes_out, 2000.0);
    }

    #[test]
    fn a_restart_or_counter_reset_gives_zero_not_a_spike() {
        let a = status(100, &[("web", 9000, 9000, 9)]);
        let b = status(1, &[("web", 10, 10, 1)]);
        assert_eq!(rates(&a, &b, 1.0).service("web"), Rate::default());
        let c = status(101, &[("web", 10, 10, 1)]);
        assert_eq!(rates(&a, &c, 1.0).service("web"), Rate::default());
    }

    #[test]
    fn history_is_bounded() {
        let mut h = History::new(3);
        for v in [1.0, 2.0, f64::NAN, 4.0, -1.0] {
            h.push(v);
        }
        assert_eq!(h.points().collect::<Vec<_>>(), vec![0.0, 4.0, 0.0]);
        assert_eq!(h.max(), 4.0);
        assert_eq!(human_bytes(512.0), "512 B/s");
        assert_eq!(human_bytes(1_250_000.0), "1.2 MB/s");
        assert_eq!(human_bytes(250_000.0), "250 KB/s");
    }
}
