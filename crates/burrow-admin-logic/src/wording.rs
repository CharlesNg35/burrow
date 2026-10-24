pub const PLAIN_NEEDS_HTTP: &str = "plain-needs-http";
pub const REDIRECT_PASSTHROUGH: &str = "redirect-passthrough";
pub const REDIRECT_PLAIN: &str = "redirect-plain";
pub const REDIRECT_NEEDS_HTTP: &str = "redirect-needs-http";
pub const NO_CERTIFICATE: &str = "no-certificate";
pub const WEB_OFF: &str = "web-off";
pub const NO_CHECK: &str = "no-check";
pub const WILDCARD_NEEDS_DNS: &str = "wildcard-needs-dns";

fn quoted(message: &str) -> &str {
    message.split('"').nth(1).unwrap_or_default()
}
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
    if code == REDIRECT_NEEDS_HTTP {
        return "answering plain HTTP too needs the web proxy's HTTP port, which is not set; add http: 80 under web in server.yaml (burrow edit), or send http:// to https://".into();
    }
    if code == PLAIN_NEEDS_HTTP {
        return "plain HTTP needs the web proxy's HTTP port, which is not set; add http: 80 under web in server.yaml (burrow edit), or choose another HTTPS option".into();
    }
    if code == NO_CHECK {
        let name = message
            .split([' ', '"'])
            .find(|w| !w.is_empty() && *w != "websocket.host")
            .unwrap_or_default();
        return format!(
            "Let's Encrypt cannot check {name}: its checks reach only ports 443 and 80; open port 80 in server.yaml (burrow edit), or use your own certificate file (DNS checks come in a later phase)"
        );
    }
    if code == WEB_OFF {
        return "the web proxy is off; turn it on in server.yaml (burrow edit) with its ports, as Settings, Web proxy shows".into();
    }
    if code == NO_CERTIFICATE {
        return format!(
            "{} has no certificate: turn on Let's Encrypt under Settings, Web proxy, choose My own certificate file under HTTPS, or list the name in the web proxy's Certificate files",
            quoted(message)
        );
    }
    if code == WILDCARD_NEEDS_DNS {
        return format!(
            "Let's Encrypt gives {} only through a DNS check, and DNS checks come in a later phase; use your own certificate file for it",
            quoted(message)
        );
    }
    if code == REDIRECT_PASSTHROUGH {
        return "the app does its own HTTPS, so burrow cannot also answer plain HTTP for it; remove this choice in server.yaml (burrow edit)".into();
    }
    if code == REDIRECT_PLAIN {
        return "a plain HTTP website cannot send visitors to https:// in this build; remove this choice in server.yaml (burrow edit)".into();
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
    fn a_name_let_s_encrypt_cannot_check_says_which_ports_it_needs() {
        for m in [
            "blog.example.com (service \"blog\") cannot be checked by Let's Encrypt: open http: 80, ...",
            "websocket.host \"blog.example.com\" cannot be checked: open http: 80, ...",
        ] {
            let shown = shown(NO_CHECK, m);
            assert!(
                shown.starts_with("Let's Encrypt cannot check blog.example.com:"),
                "{shown}"
            );
        }
    }

    #[test]
    fn a_website_with_the_web_proxy_off_says_where_to_turn_it_on() {
        let m = shown(
            WEB_OFF,
            "services.blog.web: the web proxy is off; add a top-level web: with its port, like web: {https: 443}",
        );
        assert!(
            m.contains("Settings, Web proxy") && !m.contains("web:"),
            "{m}"
        );
    }

    #[test]
    fn a_name_without_a_certificate_says_how_to_get_one_in_the_ui() {
        let m = shown(
            NO_CERTIFICATE,
            "host \"nas.example.com\" (service \"nas\") has no certificate; add web.letsencrypt for automatic certificates, tls: {cert, key} on service \"nas\", or an entry in web.certificates whose hosts lists it",
        );
        assert!(
            m.starts_with("nas.example.com has no certificate: turn on Let's Encrypt"),
            "{m}"
        );
        assert!(!m.contains("web.") && !m.contains("tls:"), "{m}");
        let w = shown(
            WILDCARD_NEEDS_DNS,
            "services.apps.web.hosts: \"*.apps.example.com\" needs DNS-01 (...)",
        );
        assert!(
            w.contains("*.apps.example.com only through a DNS check"),
            "{w}"
        );
    }

    #[test]
    fn the_redirect_choices_say_what_to_do_in_plain_words() {
        let too = shown(
            REDIRECT_NEEDS_HTTP,
            "services.nas.web.https_redirect: false serves plain HTTP too, but web.http is not set; add http: 80",
        );
        assert!(too.starts_with("answering plain HTTP too"), "{too}");
        for code in [REDIRECT_PASSTHROUGH, REDIRECT_PLAIN] {
            let m = shown(code, "services.nas.web.https_redirect: ...");
            assert!(
                !m.contains("https_redirect") && !m.contains("services."),
                "{m}"
            );
        }
    }

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
        assert_eq!(
            shown(
                PLAIN_NEEDS_HTTP,
                "services.wiki.web.tls: off serves plain HTTP, but web.http is not set; add http: 80"
            ),
            "plain HTTP needs the web proxy's HTTP port, which is not set; add http: 80 under web in server.yaml (burrow edit), or choose another HTTPS option"
        );
    }
}
