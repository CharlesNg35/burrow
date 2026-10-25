use burrow_admin_proto::codes::{
    BALANCE_TCP_ONLY, HEALTH_CHECK_TCP_ONLY, MAX_CONNECTIONS_TCP_ONLY, NO_CERTIFICATE, NO_CHECK,
    NO_HTTP, PLAIN_HTTP, PLAIN_NEEDS_HTTP, PROXY_PROTOCOL_TCP_ONLY, REDIRECT_NEEDS_HTTP,
    REDIRECT_PASSTHROUGH, REDIRECT_PLAIN, WEB_OFF, WEB_TCP_ONLY, WILDCARD_NEEDS_DNS,
};

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

pub fn help_shown(code: &str, help: Option<&str>) -> Option<String> {
    if code == NO_HTTP {
        return Some("Set the HTTP port (80) in Settings, Web proxy so http:// addresses are sent to https://".into());
    }
    help.map(str::to_string)
}

pub fn field_for(code: &str) -> Option<Vec<String>> {
    use burrow_config::{ServerConfig, WebConfig};
    (code == NO_HTTP).then(|| ServerConfig::WEB.then(WebConfig::HTTP).path())
}

pub fn shown(code: &str, message: &str, subjects: &[String]) -> String {
    let subject = subjects.first().map_or("", String::as_str);
    if code == REDIRECT_NEEDS_HTTP {
        return "answering plain HTTP too needs the web proxy's HTTP port, which is not set; set the HTTP port (80) in Settings, Web proxy, or send http:// to https://".into();
    }
    if code == PLAIN_NEEDS_HTTP {
        return "plain HTTP needs the web proxy's HTTP port, which is not set; set the HTTP port (80) in Settings, Web proxy, or choose another HTTPS option".into();
    }
    if code == NO_CHECK {
        return format!(
            "Let's Encrypt cannot check {subject}: its TLS and HTTP checks reach only ports 443 and 80; set the HTTPS port to 443 or the HTTP port to 80 in Settings, Web proxy, use the DNS check (a DNS provider under Settings, Web proxy, Let's Encrypt), or use your own certificate file"
        );
    }
    if code == WEB_OFF {
        return "the web proxy is off; turn it on with its ports in Settings, Web proxy".into();
    }
    if code == NO_CERTIFICATE {
        return format!(
            "{subject} has no certificate: turn on Let's Encrypt under Settings, Web proxy, choose My own certificate under HTTPS, or list the name in the web proxy's Certificate files"
        );
    }
    if code == WILDCARD_NEEDS_DNS {
        return format!(
            "Let's Encrypt gives {subject} only through a DNS check: add a DNS provider for its domain under Settings, Web proxy, Let's Encrypt, or use your own certificate file for it"
        );
    }
    if code == PLAIN_HTTP {
        let service = Some(subject)
            .filter(|s| !s.is_empty())
            .unwrap_or("this website");
        return format!(
            "{service} is served as plain HTTP: anyone on the path can read and change its traffic"
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAction {
    Add,
    Edit,
}

pub fn needs_write_admin(action: WriteAction, visitor: Option<&str>) -> String {
    let what = match action {
        WriteAction::Add => "Adding",
        WriteAction::Edit => "Editing",
    };
    match visitor.filter(|v| !v.is_empty()) {
        Some(v) => format!("{what} needs write admin. {v} has read."),
        None => format!("{what} needs write admin."),
    }
}

pub const CHANGE_IN_FILE: &str = "Change this in server.yaml (burrow edit)";
pub const CHANGE_IN_FILE_THEN_RESTART: &str =
    "Change this in server.yaml (burrow edit), then restart the server";
pub const TOGGLE_IN_FILE: &str = "Turn it on or off in server.yaml (burrow edit)";
pub const NAMED_IN_FILE: &str =
    "named in server.yaml (burrow edit); remove it there to upload one here";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_admin_is_told_once_in_one_wording_why_a_change_is_off() {
        assert_eq!(
            needs_write_admin(WriteAction::Add, None),
            "Adding needs write admin."
        );
        assert_eq!(
            needs_write_admin(WriteAction::Edit, Some("alice")),
            "Editing needs write admin. alice has read."
        );
        assert_eq!(
            needs_write_admin(WriteAction::Edit, Some("")),
            "Editing needs write admin."
        );
    }

    #[test]
    fn a_name_let_s_encrypt_cannot_check_says_which_ports_it_needs() {
        for m in [
            "blog.example.com (service \"blog\") cannot be checked by Let's Encrypt: open http: 80, ...",
            "websocket.host \"blog.example.com\" cannot be checked: open http: 80, ...",
        ] {
            let shown = shown(NO_CHECK, m, &["blog.example.com".into()]);
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
            "services.blog.web: the web proxy is off; add a top-level web: with its ports, like web: {https: 443, http: 80}",
            &[],
        );
        assert!(
            m.contains("Settings, Web proxy") && !m.contains("web:"),
            "{m}"
        );
    }

    #[test]
    fn a_plain_http_website_is_named_in_its_warning() {
        let item = |name: &str| {
            shown(
                PLAIN_HTTP,
                &format!(
                    "services.{name}: served as plain HTTP; anyone on the path can read and change its traffic"
                ),
                &[name.to_string()],
            )
        };
        assert_eq!(
            item("wiki"),
            "wiki is served as plain HTTP: anyone on the path can read and change its traffic"
        );
        assert_ne!(item("wiki"), item("dashboard"));
    }

    #[test]
    fn a_name_without_a_certificate_says_how_to_get_one_in_the_ui() {
        let m = shown(
            NO_CERTIFICATE,
            "host \"nas.example.com\" (service \"nas\") has no certificate; add web.letsencrypt for automatic certificates, tls: {cert, key} on service \"nas\", or an entry in web.certificates whose hosts lists it",
            &["nas.example.com".into()],
        );
        assert!(
            m.starts_with("nas.example.com has no certificate: turn on Let's Encrypt"),
            "{m}"
        );
        assert!(!m.contains("web.") && !m.contains("tls:"), "{m}");
        let w = shown(
            WILDCARD_NEEDS_DNS,
            "services.apps.web.hosts: \"*.apps.example.com\" needs DNS-01 (...)",
            &["*.apps.example.com".into()],
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
            &[],
        );
        assert!(too.starts_with("answering plain HTTP too"), "{too}");
        for code in [REDIRECT_PASSTHROUGH, REDIRECT_PLAIN] {
            let m = shown(code, "services.nas.web.https_redirect: ...", &[]);
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
            assert_eq!(shown(code, cli, &[]), ui, "{code}");
            assert!(
                !shown(code, cli, &[]).contains('_'),
                "{code}: no config keys"
            );
        }
    }

    #[test]
    fn a_message_loses_its_config_key_path_but_keeps_its_words() {
        assert_eq!(
            shown(
                "invalid-host",
                "services.nas.web.hosts: \"x\" is not a host name",
                &[],
            ),
            "\"x\" is not a host name"
        );
        assert_eq!(
            shown(
                "tunnel-host",
                "websocket.host: \"*.a.b\" is a wildcard",
                &[]
            ),
            "\"*.a.b\" is a wildcard"
        );
        assert_eq!(
            shown(
                "duplicate-host",
                "host \"a.example.com\" is used by services \"x\" and \"y\"",
                &[],
            ),
            "host \"a.example.com\" is used by services \"x\" and \"y\""
        );
        assert_eq!(
            shown("x", "invalid bind address for service \"web\": bad", &[]),
            "invalid bind address for service \"web\": bad"
        );
        assert_eq!(
            shown(
                WEB_TCP_ONLY,
                "service \"dns\" is udp, but web only works for tcp services; remove web:, or set protocol: tcp",
                &[],
            ),
            "a website only works for TCP services; set Protocol to TCP, or remove the website"
        );
        assert_eq!(
            shown(
                PLAIN_NEEDS_HTTP,
                "services.wiki.web.tls: off serves plain HTTP, but web.http is not set; add http: 80",
                &[],
            ),
            "plain HTTP needs the web proxy's HTTP port, which is not set; set the HTTP port (80) in Settings, Web proxy, or choose another HTTPS option"
        );
        assert_eq!(
            help_shown(
                NO_HTTP,
                Some("add http: 80 so http:// addresses are sent to https://")
            )
            .as_deref(),
            Some(
                "Set the HTTP port (80) in Settings, Web proxy so http:// addresses are sent to https://"
            ),
            "the UI says where, not the config syntax"
        );
        assert_eq!(
            field_for(NO_HTTP),
            Some(vec!["web".to_string(), "http".to_string()])
        );
        assert_eq!(help_shown("other", Some("x")).as_deref(), Some("x"));
    }
}
