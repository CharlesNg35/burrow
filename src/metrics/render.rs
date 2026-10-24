use std::fmt::Write;

use super::Status;

fn label(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

struct Family<'a> {
    out: &'a mut String,
}

impl Family<'_> {
    fn head(&mut self, name: &str, kind: &str, help: &str) -> &mut Self {
        let _ = writeln!(self.out, "# HELP {name} {help}");
        let _ = writeln!(self.out, "# TYPE {name} {kind}");
        self
    }

    fn line(&mut self, name: &str, labels: &[(&str, &str)], value: u64) -> &mut Self {
        let labels: Vec<String> = labels
            .iter()
            .map(|(k, v)| format!("{k}=\"{}\"", label(v)))
            .collect();
        let labels = if labels.is_empty() {
            String::new()
        } else {
            format!("{{{}}}", labels.join(","))
        };
        let _ = writeln!(self.out, "{name}{labels} {value}");
        self
    }
}

pub fn prometheus(s: &Status) -> String {
    let mut out = String::new();
    let mut f = Family { out: &mut out };
    server_families(&mut f, s);
    client_families(&mut f, s);
    service_families(&mut f, s);
    visitor_families(&mut f, s);
    out
}

fn server_families(f: &mut Family, s: &Status) {
    f.head(
        "burrow_info",
        "gauge",
        "Version of the running burrow server",
    )
    .line("burrow_info", &[("version", &s.version)], 1);
    f.head(
        "burrow_uptime_seconds",
        "gauge",
        "Seconds since the server started",
    )
    .line("burrow_uptime_seconds", &[], s.uptime_secs);
    f.head(
        "burrow_handshakes_refused_total",
        "counter",
        "Connections refused before or during authentication",
    )
    .line("burrow_handshakes_refused_total", &[], s.handshakes_refused);
    f.head(
        "burrow_udp_peers",
        "gauge",
        "UDP peers with an open session",
    )
    .line("burrow_udp_peers", &[], s.udp_peers);
}

fn client_families(f: &mut Family, s: &Status) {
    f.head(
        "burrow_client_connected",
        "gauge",
        "1 when the client is connected",
    );
    for c in &s.clients {
        f.line(
            "burrow_client_connected",
            &[("client", &c.name)],
            u64::from(c.connected),
        );
    }
    f.head(
        "burrow_client_connections_open",
        "gauge",
        "Connections currently forwarded through the client",
    );
    for c in &s.clients {
        f.line(
            "burrow_client_connections_open",
            &[("client", &c.name)],
            c.open,
        );
    }

    f.head(
        "burrow_client_busy_total",
        "counter",
        "Connections the client answered busy, at its max_connections",
    );
    for c in &s.clients {
        f.line(
            "burrow_client_busy_total",
            &[("client", &c.name)],
            c.busy_total,
        );
    }
}

fn service_families(f: &mut Family, s: &Status) {
    f.head(
        "burrow_service_connections_open",
        "gauge",
        "Connections currently open to the service",
    );
    for v in &s.services {
        f.line(
            "burrow_service_connections_open",
            &[("service", &v.name)],
            v.open,
        );
    }
    f.head(
        "burrow_service_connections_total",
        "counter",
        "Connections accepted by the service",
    );
    for v in &s.services {
        f.line(
            "burrow_service_connections_total",
            &[("service", &v.name)],
            v.total,
        );
    }
    f.head(
        "burrow_service_refused_total",
        "counter",
        "Connections the service refused: source address, limits, or no client",
    );
    for v in &s.services {
        f.line(
            "burrow_service_refused_total",
            &[("service", &v.name)],
            v.refused,
        );
    }
    f.head(
        "burrow_service_bytes_total",
        "counter",
        "Bytes forwarded; in is from users to the target, out is back",
    );
    for v in &s.services {
        f.line(
            "burrow_service_bytes_total",
            &[("service", &v.name), ("direction", "in")],
            v.bytes_in,
        )
        .line(
            "burrow_service_bytes_total",
            &[("service", &v.name), ("direction", "out")],
            v.bytes_out,
        );
    }
    f.head(
        "burrow_service_client_up",
        "gauge",
        "Health check result: 1 when the client reaches the service target",
    );
    for v in &s.services {
        for h in &v.health {
            f.line(
                "burrow_service_client_up",
                &[("service", &v.name), ("client", &h.client)],
                u64::from(h.up),
            );
        }
    }
}

fn visitor_families(f: &mut Family, s: &Status) {
    f.head(
        "burrow_visitor_connections_open",
        "gauge",
        "Connections currently open by the visitor",
    );
    for v in &s.visitors {
        f.line(
            "burrow_visitor_connections_open",
            &[("visitor", &v.name)],
            v.open,
        );
    }
    f.head(
        "burrow_visitor_connections_total",
        "counter",
        "Connections made by the visitor",
    );
    for v in &s.visitors {
        f.line(
            "burrow_visitor_connections_total",
            &[("visitor", &v.name)],
            v.total,
        );
    }
}

pub fn duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        3600..86400 => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
        _ => format!("{} d {} h", secs / 86400, secs % 86400 / 3600),
    }
}

pub fn bytes(n: u64) -> String {
    let n = n as f64;
    for (unit, size) in [("TB", 1e12), ("GB", 1e9), ("MB", 1e6), ("KB", 1e3)] {
        if n >= size {
            return format!("{:.1} {unit}", n / size);
        }
    }
    format!("{n} B")
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn sample() -> Status {
        Status {
            version: "9.9.9".into(),
            uptime_secs: 3 * 86400 + 4 * 3600,
            listening: vec![],
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
                    ..ClientStatus::default()
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
                    ..ClientStatus::default()
                },
            ],
            services: vec![ServiceStatus {
                name: "web".into(),
                protocol: "tcp".into(),
                bind: Some("0.0.0.0:443".into()),
                target: "127.0.0.1:8443".into(),
                clients: vec!["home".into(), "office".into()],
                mode: None,
                bandwidth: None,
                visitors: vec![],
                proxy_protocol: None,
                filtered: false,
                open: 12,
                total: 153,
                refused: 1,
                bytes_in: 1_200_000_000,
                bytes_out: 8_400_000_000,
                health: vec![
                    HealthStatus {
                        client: "home".into(),
                        up: true,
                    },
                    HealthStatus {
                        client: "office".into(),
                        up: false,
                    },
                ],
                set_aside: vec![],
            }],
            visitors: vec![VisitorStatus {
                name: "lap\"top".into(),
                open: 1,
                total: 12,
                last_seen_secs: Some(0),
            }],
            web: None,
        }
    }

    #[test]
    fn prometheus_text() {
        let text = prometheus(&sample());
        for line in [
            "burrow_info{version=\"9.9.9\"} 1",
            "burrow_client_connected{client=\"office\"} 0",
            "burrow_service_bytes_total{service=\"web\",direction=\"out\"} 8400000000",
            "burrow_service_client_up{service=\"web\",client=\"office\"} 0",
            "burrow_visitor_connections_total{visitor=\"lap\\\"top\"} 12",
            "# TYPE burrow_service_connections_total counter",
        ] {
            assert!(
                text.lines().any(|l| l == line),
                "missing {line:?} in\n{text}"
            );
        }
        for l in text.lines().filter(|l| !l.starts_with('#')) {
            let value = l.rsplit(' ').next().unwrap();
            assert!(value.parse::<u64>().is_ok(), "{l}");
        }
    }

    #[test]
    fn sizes_and_times() {
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(3_100_000), "3.1 MB");
        assert_eq!(duration(59), "59 s");
        assert_eq!(duration(61), "1 min");
    }
}
