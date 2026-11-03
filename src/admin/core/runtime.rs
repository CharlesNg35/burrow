use std::net::SocketAddr;

use burrow_config::ServerConfig;

pub(super) fn not_open(
    file: &crate::config::settings::ServerSettings,
    running: &crate::config::settings::ServerSettings,
    status: &crate::metrics::Status,
) -> Vec<burrow_admin_proto::msg::NotOpen> {
    use burrow_config::{MetricsConfig, WebConfig, WebSocketConfig};
    let shown = |a: Option<SocketAddr>| a.map(|a| a.to_string()).unwrap_or_default();
    let web =
        |s: &crate::config::settings::ServerSettings| s.web.as_ref().map(|w| (w.https, w.http));
    let waiting = |key: &str| status.web_waiting.as_ref().is_some_and(|w| w.key == key);
    let (fw, rw) = (
        web(file).unwrap_or_default(),
        web(running).unwrap_or_default(),
    );
    [
        (
            ServerConfig::WEBSOCKET.then(WebSocketConfig::LISTEN).path(),
            file.websocket.as_ref().map(|w| w.listen),
            running.websocket.as_ref().map(|w| w.listen),
        ),
        (
            ServerConfig::METRICS.then(MetricsConfig::LISTEN).path(),
            file.metrics,
            running.metrics,
        ),
        (ServerConfig::WEB.then(WebConfig::HTTPS).path(), fw.0, rw.0),
        (ServerConfig::WEB.then(WebConfig::HTTP).path(), fw.1, rw.1),
    ]
    .into_iter()
    .filter(|(p, f, r)| f.is_some() && f != r && !waiting(&p.join(".")))
    .map(|(path, f, r)| burrow_admin_proto::msg::NotOpen {
        path,
        wanted: shown(f),
        running: shown(r),
    })
    .collect()
}

pub(super) fn addresses(s: &crate::config::settings::ServerSettings) -> Vec<String> {
    let mut out = vec![
        "0.0.0.0".to_string(),
        "::".to_string(),
        "127.0.0.1".to_string(),
    ];
    let mut add = |ip: std::net::IpAddr| {
        let t = ip.to_string();
        if !out.contains(&t) {
            out.push(t);
        }
    };
    if let Some(l) = s.listen {
        add(l.ip());
    }
    if let Some(w) = &s.websocket {
        add(w.listen.ip());
    }
    for svc in s.services.values() {
        if let Some(b) = svc.bind {
            add(b.ip());
        }
    }
    if let Some(ip) = s
        .public_address
        .as_deref()
        .and_then(|a| a.parse::<std::net::IpAddr>().ok())
    {
        add(ip);
    }
    out
}
