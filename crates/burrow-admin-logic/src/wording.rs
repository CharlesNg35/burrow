pub const WEB_HTTPS_LATER: &str = "web-https-later";
pub const WEB_HTTP_LATER: &str = "web-http-later";
pub const WEB_TLS_FILES_LATER: &str = "web-tls-files-later";
pub const WEB_TCP_ONLY: &str = "web-tcp-only";
pub const PROXY_PROTOCOL_TCP_ONLY: &str = "proxy-protocol-tcp-only";
pub const MAX_CONNECTIONS_TCP_ONLY: &str = "max-connections-tcp-only";
pub const BALANCE_TCP_ONLY: &str = "balance-tcp-only";
pub const HEALTH_CHECK_TCP_ONLY: &str = "health-check-tcp-only";

const TCP_ONLY: [(&str, &str); 5] = [
    (
        WEB_TCP_ONLY,
        "a website only works for TCP services; set Protocol to TCP, or remove the website",
    ),
    (
        PROXY_PROTOCOL_TCP_ONLY,
        "PROXY protocol only works for TCP services; set Protocol to TCP, or clear it",
    ),
    (
        MAX_CONNECTIONS_TCP_ONLY,
        "Max connections only works for TCP services; set Protocol to TCP, or clear it (for UDP, Max UDP peers per port under Settings limits the peers of each port)",
    ),
    (
        BALANCE_TCP_ONLY,
        "Balance only works for TCP services; set Protocol to TCP, or choose Failover",
    ),
    (
        HEALTH_CHECK_TCP_ONLY,
        "Health check only works for TCP services; set Protocol to TCP, or turn it off",
    ),
];

fn key_path(head: &str) -> bool {
    head.contains('.')
        && head
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '*'))
}

pub fn shown(code: &str, message: &str) -> String {
    if code == WEB_HTTPS_LATER || code == WEB_TLS_FILES_LATER {
        return "burrow cannot serve HTTPS for websites in this build yet; under HTTPS, choose The app does its own HTTPS".into();
    }
    if code == WEB_HTTP_LATER {
        return "plain HTTP websites are not in this build yet; under HTTPS, choose The app does its own HTTPS".into();
    }
    if let Some((_, text)) = TCP_ONLY.iter().find(|(c, _)| *c == code) {
        return (*text).into();
    }
    match message.split_once(": ") {
        Some((head, rest)) if key_path(head) => rest.to_string(),
        _ => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tcp_only_field_on_a_udp_service_says_its_label_and_the_fix() {
        let cases = [
            (
                PROXY_PROTOCOL_TCP_ONLY,
                "service \"web\" is udp, but proxy_protocol only works for tcp services",
                "PROXY protocol only works for TCP services; set Protocol to TCP, or clear it",
            ),
            (
                MAX_CONNECTIONS_TCP_ONLY,
                "service \"web\" is udp, but max_connections only works for tcp services; for UDP, udp.max_peers limits the peers of each port",
                "Max connections only works for TCP services; set Protocol to TCP, or clear it (for UDP, Max UDP peers per port under Settings limits the peers of each port)",
            ),
            (
                BALANCE_TCP_ONLY,
                "service \"web\" is udp, but mode balance only works for tcp services",
                "Balance only works for TCP services; set Protocol to TCP, or choose Failover",
            ),
            (
                HEALTH_CHECK_TCP_ONLY,
                "service \"web\" is udp, but health_check only works for tcp services",
                "Health check only works for TCP services; set Protocol to TCP, or turn it off",
            ),
        ];
        for (code, cli, ui) in cases {
            assert_eq!(shown(code, cli), ui, "{code}");
            assert!(!shown(code, cli).contains('_'), "{code}: no config keys");
        }
    }

    #[test]
    fn a_message_loses_its_config_key_path_but_keeps_its_words() {
        assert_eq!(
            shown(
                "invalid-host",
                "services.nas.web.hosts: \"x\" is not a host name"
            ),
            "\"x\" is not a host name"
        );
        assert_eq!(
            shown("tunnel-host", "websocket.host: \"*.a.b\" is a wildcard"),
            "\"*.a.b\" is a wildcard"
        );
        assert_eq!(
            shown(
                "duplicate-host",
                "host \"a.example.com\" is used by services \"x\" and \"y\""
            ),
            "host \"a.example.com\" is used by services \"x\" and \"y\""
        );
        assert_eq!(
            shown("x", "invalid bind address for service \"web\": bad"),
            "invalid bind address for service \"web\": bad"
        );
        assert_eq!(
            shown(
                WEB_TCP_ONLY,
                "service \"dns\" is udp, but web only works for tcp services; remove web:, or set protocol: tcp"
            ),
            "a website only works for TCP services; set Protocol to TCP, or remove the website"
        );
        assert!(
            shown(WEB_HTTPS_LATER, "services.nas.web: ... tls: passthrough")
                .contains("choose The app does its own HTTPS")
        );
        assert_eq!(
            shown(WEB_TLS_FILES_LATER, "services.nas.web.tls: ..."),
            shown(WEB_HTTPS_LATER, "services.nas.web.tls: ...")
        );
    }
}
