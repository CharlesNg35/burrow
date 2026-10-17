use crate::config::settings::ClientSettings;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub reconnect: Vec<&'static str>,
    pub live: Vec<&'static str>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.reconnect.is_empty() && self.live.is_empty()
    }

    pub fn describe(&self) -> String {
        let all = |names: &[&str]| names.join(", ");
        match (self.reconnect.is_empty(), self.live.is_empty()) {
            (true, true) => "config reloaded: nothing changed".into(),
            (true, false) => format!("config reloaded: {} applied", all(&self.live)),
            (false, _) => {
                let mut names = self.reconnect.clone();
                names.extend(&self.live);
                let why = if self.reconnect == ["allow"] {
                    "so the server stops skipping targets this client refused before"
                } else {
                    "to use the new connection settings"
                };
                format!(
                    "config reloaded: {} changed; reconnecting {why}",
                    all(&names)
                )
            }
        }
    }
}

pub fn compare(old: &ClientSettings, new: &ClientSettings) -> Changes {
    let mut c = Changes::default();
    let mut reconnect = |changed: bool, name: &'static str| {
        if changed {
            c.reconnect.push(name);
        }
    };
    reconnect(
        old.endpoint != new.endpoint || old.server != new.server,
        "server",
    );
    reconnect(old.name != new.name, "name");
    reconnect(old.secret != new.secret, "secret");
    reconnect(old.headers != new.headers, "headers");
    reconnect(old.ca_pem != new.ca_pem, "ca");
    reconnect(old.cipher != new.cipher, "cipher");
    reconnect(old.proxy != new.proxy, "proxy");
    reconnect(
        old.insecure_plaintext != new.insecure_plaintext,
        "insecure_plaintext",
    );
    reconnect(old.allow != new.allow, "allow");
    let mut live = |changed: bool, name: &'static str| {
        if changed {
            c.live.push(name);
        }
    };
    live(
        old.reconnect_initial != new.reconnect_initial
            || old.reconnect_max != new.reconnect_max
            || old.give_up_after != new.give_up_after
            || old.exit_on_rejected != new.exit_on_rejected
            || old.reject_confirm != new.reject_confirm,
        "reconnect",
    );
    live(
        old.connect_timeout != new.connect_timeout,
        "connect_timeout_secs",
    );
    live(old.tcp != new.tcp, "tcp");
    live(old.udp_session_timeout != new.udp_session_timeout, "udp");
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allowlist::AllowList;
    use crate::config::model::Cipher;
    use crate::config::settings::SocketTuning;
    use crate::secret::Secret;
    use crate::transport::endpoint::Endpoint;
    use std::time::Duration;

    fn settings() -> ClientSettings {
        ClientSettings {
            server: "tunnel.example.com".into(),
            endpoint: Endpoint::parse("tunnel.example.com").unwrap(),
            headers: Vec::new(),
            ca_pem: None,
            ca_file: None,
            name: "home".into(),
            secret: Secret::generate(),
            allow: AllowList::Rules(vec!["127.0.0.0/8".parse().unwrap()]),
            insecure_plaintext: false,
            cipher: Cipher::Chachapoly,
            proxy: None,
            reconnect_initial: Duration::from_secs(1),
            reconnect_max: Duration::from_secs(30),
            give_up_after: None,
            exit_on_rejected: false,
            reject_confirm: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(10),
            tcp: SocketTuning {
                nodelay: true,
                keepalive: None,
            },
            udp_session_timeout: Duration::from_secs(60),
        }
    }

    #[test]
    fn connection_settings_and_the_allow_list_reconnect_the_rest_applies_live() {
        let old = settings();
        assert!(compare(&old, &old).is_empty());
        let mut new = old.clone();
        new.connect_timeout = Duration::from_secs(5);
        new.reconnect_max = Duration::from_secs(60);
        assert_eq!(
            compare(&old, &new),
            Changes {
                reconnect: vec![],
                live: vec!["reconnect", "connect_timeout_secs"],
            }
        );
        new.allow = AllowList::Rules(vec!["10.0.0.0/8".parse().unwrap()]);
        assert_eq!(compare(&old, &new).reconnect, vec!["allow"]);
        new.secret = Secret::generate();
        new.server = "other.example.com".into();
        new.endpoint = Endpoint::parse("other.example.com").unwrap();
        assert_eq!(
            compare(&old, &new).reconnect,
            vec!["server", "secret", "allow"]
        );
    }

    #[test]
    fn the_log_line_says_what_changed_and_what_happens() {
        let old = settings();
        let mut new = old.clone();
        assert_eq!(
            compare(&old, &new).describe(),
            "config reloaded: nothing changed"
        );
        new.tcp.nodelay = false;
        assert_eq!(
            compare(&old, &new).describe(),
            "config reloaded: tcp applied"
        );
        new.allow = AllowList::Rules(vec![]);
        assert_eq!(
            compare(&old, &new).describe(),
            "config reloaded: allow, tcp changed; reconnecting so the server stops skipping targets this client refused before"
        );
        new.name = "office".into();
        assert!(
            compare(&old, &new)
                .describe()
                .ends_with("to use the new connection settings")
        );
    }
}
