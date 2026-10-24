pub const WEB_HTTPS_LATER: &str = "web-https-later";
pub const WEB_HTTP_LATER: &str = "web-http-later";

fn key_path(head: &str) -> bool {
    head.contains('.')
        && head
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '*'))
}

pub fn shown(code: &str, message: &str) -> String {
    if code == WEB_HTTPS_LATER {
        return "burrow cannot serve HTTPS for websites in this build yet; under HTTPS, choose The app does its own HTTPS".into();
    }
    if code == WEB_HTTP_LATER {
        return "plain HTTP websites are not in this build yet; under HTTPS, choose The app does its own HTTPS".into();
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
        assert!(
            shown(WEB_HTTPS_LATER, "services.nas.web: ... tls: passthrough")
                .contains("choose The app does its own HTTPS")
        );
    }
}
