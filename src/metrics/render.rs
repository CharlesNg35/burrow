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
    web_families(&mut f, s);
    visitor_families(&mut f, s);
    certificate_families(&mut f, s);
    out
}

fn certificate_families(f: &mut Family, s: &Status) {
    use burrow_admin_proto::status::CertSourceStatus as Source;
    let served: Vec<_> = s
        .certificates
        .iter()
        .filter(|c| !matches!(c.source, Source::Passthrough | Source::Plain))
        .collect();
    if served.is_empty() {
        return;
    }
    f.head(
        "burrow_certificate_expiry_timestamp_seconds",
        "gauge",
        "When the certificate served for a web name expires, in Unix seconds.",
    );
    for c in served.iter().filter(|c| c.expires.is_some()) {
        f.line(
            "burrow_certificate_expiry_timestamp_seconds",
            &[("name", &c.name)],
            c.expires.unwrap_or_default(),
        );
    }
    f.head(
        "burrow_certificate_failures_total",
        "counter",
        "Failed attempts to get or renew the certificate for a web name since the server started.",
    );
    for c in &served {
        f.line(
            "burrow_certificate_failures_total",
            &[("name", &c.name)],
            c.failures,
        );
    }
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

fn web_families(f: &mut Family, s: &Status) {
    let sites: Vec<_> = s
        .services
        .iter()
        .filter_map(|v| v.http.as_ref().map(|h| (v, h)))
        .collect();
    if sites.is_empty() {
        return;
    }
    f.head(
        "burrow_web_responses_total",
        "counter",
        "HTTP responses a website sent, by status class",
    );
    for (v, h) in sites {
        for (class, n) in ["1xx", "2xx", "3xx", "4xx", "5xx"].iter().zip(h.responses) {
            f.line(
                "burrow_web_responses_total",
                &[("service", &v.name), ("class", class)],
                n,
            );
        }
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
    use burrow_admin_proto::status::CertSourceStatus;

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
                        target: None,
                        up: true,
                        reason: None,
                    },
                    HealthStatus {
                        client: "office".into(),
                        target: None,
                        up: false,
                        reason: None,
                    },
                ],
                set_aside: vec![],
                entries: vec![],
                web: vec!["example.com".into()],
                http: Some(burrow_admin_proto::status::HttpCounts {
                    responses: [0, 140, 3, 9, 1],
                }),
            }],
            visitors: vec![VisitorStatus {
                name: "lap\"top".into(),
                open: 1,
                total: 12,
                last_seen_secs: Some(0),
            }],
            web: None,
            certificates: vec![
                cert(
                    "blog.example.com",
                    CertSourceStatus::LetsEncrypt {
                        directory: "d".into(),
                    },
                    Some(1_800_000_000),
                    2,
                ),
                cert("nas.example.com", CertSourceStatus::Passthrough, None, 0),
                cert(
                    "new.example.com",
                    CertSourceStatus::LetsEncrypt {
                        directory: "d".into(),
                    },
                    None,
                    0,
                ),
            ],
            acme: None,
            local_ca: None,
            web_waiting: None,
            notify: Vec::new(),
        }
    }

    fn cert(
        name: &str,
        source: CertSourceStatus,
        expires: Option<u64>,
        failures: u64,
    ) -> CertStatus {
        CertStatus {
            name: name.into(),
            source,
            check: None,
            state: burrow_admin_proto::status::CertStateStatus::Ok,
            expires,
            next_renewal: None,
            last_error: None,
            code: None,
            explanation: None,
            failures,
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
            "burrow_certificate_expiry_timestamp_seconds{name=\"blog.example.com\"} 1800000000",
            "burrow_certificate_failures_total{name=\"blog.example.com\"} 2",
            "burrow_certificate_failures_total{name=\"new.example.com\"} 0",
            "# TYPE burrow_certificate_failures_total counter",
            "# TYPE burrow_web_responses_total counter",
            "burrow_web_responses_total{service=\"web\",class=\"2xx\"} 140",
            "burrow_web_responses_total{service=\"web\",class=\"5xx\"} 1",
            "burrow_web_responses_total{service=\"web\",class=\"1xx\"} 0",
        ] {
            assert!(
                text.lines().any(|l| l == line),
                "missing {line:?} in\n{text}"
            );
        }
        assert!(
            !text.contains("nas.example.com"),
            "passthrough names have no certificate of burrow's"
        );
        assert!(!text.contains("expiry_timestamp_seconds{name=\"new.example.com\""));
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
