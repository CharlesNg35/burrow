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

pub const RECORD_SECONDS: usize = 900;

#[derive(Debug, Clone, PartialEq)]
pub struct History {
    cap: usize,
    points: std::collections::VecDeque<Option<f64>>,
}

impl History {
    pub fn new(cap: usize) -> Self {
        History {
            cap: cap.max(1),
            points: std::collections::VecDeque::with_capacity(cap.max(1)),
        }
    }

    fn put(&mut self, v: Option<f64>) {
        if self.points.len() == self.cap {
            self.points.pop_front();
        }
        self.points.push_back(v);
    }

    pub fn push(&mut self, v: f64) {
        self.put(Some(if v.is_finite() { v.max(0.0) } else { 0.0 }));
    }

    pub fn gap(&mut self) {
        self.put(None);
    }

    pub fn points(&self) -> impl Iterator<Item = Option<f64>> + '_ {
        self.points.iter().copied()
    }

    pub fn last(&self) -> Option<f64> {
        self.points.back().copied().flatten()
    }

    pub fn max(&self) -> f64 {
        self.points.iter().flatten().copied().fold(0.0, f64::max)
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn samples(&self) -> usize {
        self.points.iter().filter(|p| p.is_some()).count()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recorder {
    pub total: (History, History),
    pub services: std::collections::BTreeMap<String, (History, History)>,
}

impl Default for Recorder {
    fn default() -> Self {
        Recorder {
            total: (History::new(RECORD_SECONDS), History::new(RECORD_SECONDS)),
            services: std::collections::BTreeMap::new(),
        }
    }
}

impl Recorder {
    pub fn record(&mut self, r: &Rates) {
        let len = self.total.0.len();
        self.total.0.push(r.total.bytes_in);
        self.total.1.push(r.total.bytes_out);
        self.services
            .retain(|name, _| r.services.iter().any(|(n, _)| n == name));
        for (name, rate) in &r.services {
            let ring = self.services.entry(name.clone()).or_insert_with(|| {
                let mut a = History::new(RECORD_SECONDS);
                let mut b = History::new(RECORD_SECONDS);
                for _ in 0..len.min(RECORD_SECONDS - 1) {
                    a.gap();
                    b.gap();
                }
                (a, b)
            });
            ring.0.push(rate.bytes_in);
            ring.1.push(rate.bytes_out);
        }
    }

    pub fn gap(&mut self) {
        self.total.0.gap();
        self.total.1.gap();
        for ring in self.services.values_mut() {
            ring.0.gap();
            ring.1.gap();
        }
    }

    pub fn series(&self, service: Option<&str>) -> Option<&(History, History)> {
        match service {
            None => Some(&self.total),
            Some(s) => self.services.get(s),
        }
    }
}

pub fn mbit(bytes_per_sec: f64) -> f64 {
    bytes_per_sec.max(0.0) * 8.0 / 1_000_000.0
}

pub fn axis_max(max_mbit: f64) -> f64 {
    let top = max_mbit.max(0.0) * 1.1;
    if top >= 20.0 {
        return (top / 20.0).ceil() * 20.0;
    }
    for step in [
        0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0,
    ] {
        if top <= step * 4.0 {
            return step * 4.0;
        }
    }
    20.0
}

pub fn human_bits(bytes_per_sec: f64) -> (String, &'static str) {
    let bits = bytes_per_sec.max(0.0) * 8.0;
    let (v, unit) = if bits >= 1e9 {
        (bits / 1e9, "Gbit/s")
    } else if bits >= 1e6 {
        (bits / 1e6, "Mbit/s")
    } else if bits >= 1e3 {
        (bits / 1e3, "kbit/s")
    } else {
        (bits, "bit/s")
    };
    let text = if v >= 100.0 || unit == "bit/s" {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    };
    (text, unit)
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
        assert_eq!(
            h.points().collect::<Vec<_>>(),
            vec![Some(0.0), Some(4.0), Some(0.0)]
        );
        assert_eq!(h.max(), 4.0);
    }

    #[test]
    fn the_recorder_keeps_fifteen_minutes_with_gaps_and_rings_per_service() {
        let mut r = Recorder::default();
        let rates = |pairs: &[(&str, f64)]| Rates {
            services: pairs
                .iter()
                .map(|(n, v)| {
                    (
                        n.to_string(),
                        Rate {
                            bytes_in: *v,
                            bytes_out: v / 2.0,
                            connections: 0.0,
                        },
                    )
                })
                .collect(),
            total: Rate {
                bytes_in: pairs.iter().map(|p| p.1).sum(),
                bytes_out: 0.0,
                connections: 0.0,
            },
        };
        r.record(&rates(&[("web", 10.0)]));
        r.gap();
        r.record(&rates(&[("web", 20.0), ("ssh", 5.0)]));
        assert_eq!(
            r.total.0.points().collect::<Vec<_>>(),
            vec![Some(10.0), None, Some(25.0)]
        );
        assert_eq!(
            r.services["ssh"].0.points().collect::<Vec<_>>(),
            vec![None, None, Some(5.0)],
            "a new ring lines up with the others"
        );
        r.record(&rates(&[("web", 1.0)]));
        assert!(
            !r.services.contains_key("ssh"),
            "a removed service drops its ring"
        );
        for _ in 0..1000 {
            r.record(&rates(&[("web", 1.0)]));
        }
        assert_eq!(r.total.0.len(), RECORD_SECONDS);
        assert_eq!(r.services["web"].0.len(), RECORD_SECONDS);
    }

    #[test]
    fn rates_read_as_bits_and_the_axis_rounds_up() {
        assert_eq!(human_bits(6_025_000.0), ("48.2".to_string(), "Mbit/s"));
        assert_eq!(human_bits(100.0), ("800".to_string(), "bit/s"));
        assert_eq!(axis_max(70.0), 80.0);
        assert_eq!(axis_max(55.0), 80.0);
        assert_eq!(axis_max(0.3), 0.4);
        assert_eq!(axis_max(0.0), 0.004);
        assert!((mbit(1_250_000.0) - 10.0).abs() < 1e-9);
    }
}
