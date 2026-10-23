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
    let ClientSettings {
        server,
        endpoint,
        headers,
        ca_pem,
        ca_file: _,
        name,
        secret,
        allow,
        insecure_plaintext,
        cipher,
        proxy,
        reconnect_initial,
        reconnect_max,
        give_up_after,
        exit_on_rejected,
        reject_confirm,
        connect_timeout,
        tcp,
        udp_session_timeout,
        max_connections,
        max_udp_peers,
    } = new;
    let reconnect = [
        (*server != old.server || *endpoint != old.endpoint, "server"),
        (*name != old.name, "name"),
        (*secret != old.secret, "secret"),
        (*headers != old.headers, "headers"),
        (*ca_pem != old.ca_pem, "ca"),
        (*cipher != old.cipher, "cipher"),
        (*proxy != old.proxy, "proxy"),
        (
            *insecure_plaintext != old.insecure_plaintext,
            "insecure_plaintext",
        ),
        (*allow != old.allow, "allow"),
    ];
    let live = [
        (
            *reconnect_initial != old.reconnect_initial
                || *reconnect_max != old.reconnect_max
                || *give_up_after != old.give_up_after
                || *exit_on_rejected != old.exit_on_rejected
                || *reject_confirm != old.reject_confirm,
            "reconnect",
        ),
        (
            *connect_timeout != old.connect_timeout,
            "connect_timeout_secs",
        ),
        (*tcp != old.tcp, "tcp"),
        (*udp_session_timeout != old.udp_session_timeout, "udp"),
        (*max_connections != old.max_connections, "max_connections"),
        (*max_udp_peers != old.max_udp_peers, "max_udp_peers"),
    ];
    let names = |fields: &[(bool, &'static str)]| {
        fields
            .iter()
            .filter_map(|(changed, name)| changed.then_some(*name))
            .collect()
    };
    Changes {
        reconnect: names(&reconnect),
        live: names(&live),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::allowlist::AllowList;
    use crate::config::settings::SocketTuning;
    use crate::secret::Secret;
    use burrow_config::Cipher;
    use burrow_syntax::endpoint::Endpoint;
    use std::time::Duration;

    pub(crate) fn settings() -> ClientSettings {
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
            max_connections: 16384,
            max_udp_peers: 8192,
        }
    }

    #[test]
    fn connection_settings_and_the_allow_list_reconnect_the_rest_applies_live() {
        let old = settings();
        assert!(compare(&old, &old).is_empty());
        let mut new = old.clone();
        new.connect_timeout = Duration::from_secs(5);
        new.reconnect_max = Duration::from_secs(60);
        new.max_connections = 3;
        new.max_udp_peers = 4;
        assert_eq!(
            compare(&old, &new),
            Changes {
                reconnect: vec![],
                live: vec![
                    "reconnect",
                    "connect_timeout_secs",
                    "max_connections",
                    "max_udp_peers"
                ],
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
    fn the_running_client_takes_every_change_and_reconnects_only_when_needed() {
        let old = settings();
        let client = crate::client::Client::new(old.clone());
        let mut backoff =
            crate::client::reconnect::Backoff::new(old.reconnect_initial, old.reconnect_max);
        let mut next = old.clone();
        next.connect_timeout = Duration::from_secs(3);
        next.tcp.nodelay = false;
        next.udp_session_timeout = Duration::from_secs(5);
        next.reconnect_max = Duration::from_secs(60);
        next.exit_on_rejected = true;
        assert!(!client.apply(next.clone(), &mut backoff));
        assert_eq!(*client.live.settings(), next);
        let mut renamed = next.clone();
        renamed.name = "office".into();
        assert!(client.apply(renamed.clone(), &mut backoff));
        assert_eq!(*client.live.settings(), renamed);
        assert_eq!(client.live.dialer().name, "office");
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
