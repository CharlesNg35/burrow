use crate::config::settings::ServerSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Applied {
    Live,
    Reload,
    Restart,
}

pub(super) fn changed(old: &ServerSettings, new: &ServerSettings) -> Vec<(&'static str, Applied)> {
    let ServerSettings {
        listen,
        public_address,
        websocket,
        clients,
        visitors,
        services,
        insecure_plaintext,
        heartbeat_interval,
        handshake_timeout,
        max_handshakes,
        max_connections_per_client,
        max_pending,
        pool_min_idle,
        pool_max_idle,
        tcp,
        udp_session_timeout,
        metrics,
    } = new;
    use Applied::{Live, Reload, Restart};
    [
        (*listen != old.listen, "listen", Restart),
        (
            *public_address != old.public_address,
            "public_address",
            Live,
        ),
        (*websocket != old.websocket, "websocket", Reload),
        (*clients != old.clients, "clients", Reload),
        (*visitors != old.visitors, "visitors", Reload),
        (*services != old.services, "services", Reload),
        (
            *insecure_plaintext != old.insecure_plaintext,
            "insecure_plaintext",
            Live,
        ),
        (
            *heartbeat_interval != old.heartbeat_interval,
            "heartbeat",
            Live,
        ),
        (
            *handshake_timeout != old.handshake_timeout,
            "limits.handshake_timeout_secs",
            Live,
        ),
        (
            *max_handshakes != old.max_handshakes,
            "limits.max_handshakes",
            Reload,
        ),
        (
            *max_connections_per_client != old.max_connections_per_client,
            "limits.max_connections_per_client",
            Live,
        ),
        (
            *max_pending != old.max_pending,
            "limits.max_pending",
            Reload,
        ),
        (*pool_min_idle != old.pool_min_idle, "pool.min_idle", Reload),
        (*pool_max_idle != old.pool_max_idle, "pool.max_idle", Reload),
        (*tcp != old.tcp, "tcp", Live),
        (*udp_session_timeout != old.udp_session_timeout, "udp", Live),
        (*metrics != old.metrics, "metrics", Reload),
    ]
    .into_iter()
    .filter_map(|(changed, name, how)| changed.then_some((name, how)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Format, Loaded, Role, load_str};
    use std::time::Duration;

    fn settings() -> ServerSettings {
        let yaml = format!(
            "listen: 127.0.0.1:7835\nclients:\n  home:\n    secret: {}\nservices:\n  web: {{ client: home, bind: 7840, target: 3000 }}\n",
            crate::secret::Secret::generate().encode()
        );
        let nothing = |_: &str| None;
        match load_str(
            "server.yaml",
            &yaml,
            Format::Yaml,
            Some(Role::Server),
            &nothing,
        )
        .loaded
        {
            Some(Loaded::Server(s, _)) => *s,
            _ => panic!("server config loads"),
        }
    }

    #[test]
    fn every_field_has_a_way_to_apply() {
        let old = settings();
        assert!(changed(&old, &old).is_empty());
        type Edit = Box<dyn Fn(&mut ServerSettings)>;
        let edits: Vec<(&str, Applied, Edit)> = vec![
            (
                "listen",
                Applied::Restart,
                Box::new(|s| s.listen = Some("127.0.0.1:7900".parse().unwrap())),
            ),
            (
                "public_address",
                Applied::Live,
                Box::new(|s| s.public_address = Some("x.example.com".into())),
            ),
            (
                "services",
                Applied::Reload,
                Box::new(|s| {
                    s.services.clear();
                }),
            ),
            (
                "clients",
                Applied::Reload,
                Box::new(|s| {
                    s.clients.clear();
                }),
            ),
            (
                "visitors",
                Applied::Reload,
                Box::new(|s| {
                    s.visitors.insert(
                        "laptop".into(),
                        crate::config::settings::VisitorAuth {
                            secrets: vec![crate::secret::Secret::generate()],
                            reach: Default::default(),
                            max_connections: None,
                        },
                    );
                }),
            ),
            (
                "websocket",
                Applied::Reload,
                Box::new(|s| {
                    s.websocket = Some(crate::config::settings::WebSocketSettings {
                        listen: "127.0.0.1:7836".parse().unwrap(),
                        path: "/burrow".into(),
                        tls: None,
                        keepalive: Duration::from_secs(25),
                    })
                }),
            ),
            (
                "insecure_plaintext",
                Applied::Live,
                Box::new(|s| s.insecure_plaintext = true),
            ),
            (
                "heartbeat",
                Applied::Live,
                Box::new(|s| s.heartbeat_interval = Duration::from_secs(99)),
            ),
            (
                "limits.handshake_timeout_secs",
                Applied::Live,
                Box::new(|s| s.handshake_timeout = Duration::from_secs(99)),
            ),
            (
                "limits.max_handshakes",
                Applied::Reload,
                Box::new(|s| s.max_handshakes = 3),
            ),
            (
                "limits.max_connections_per_client",
                Applied::Live,
                Box::new(|s| s.max_connections_per_client = 3),
            ),
            (
                "limits.max_pending",
                Applied::Reload,
                Box::new(|s| s.max_pending = 3),
            ),
            (
                "pool.min_idle",
                Applied::Reload,
                Box::new(|s| s.pool_min_idle = 9),
            ),
            (
                "pool.max_idle",
                Applied::Reload,
                Box::new(|s| s.pool_max_idle = 99),
            ),
            (
                "tcp",
                Applied::Live,
                Box::new(|s| s.tcp.nodelay = !s.tcp.nodelay),
            ),
            (
                "udp",
                Applied::Live,
                Box::new(|s| s.udp_session_timeout = Duration::from_secs(9)),
            ),
            (
                "metrics",
                Applied::Reload,
                Box::new(|s| s.metrics = Some("127.0.0.1:9100".parse().unwrap())),
            ),
        ];
        for (name, how, edit) in edits {
            let mut new = old.clone();
            edit(&mut new);
            assert_eq!(changed(&old, &new), vec![(name, how)], "{name}");
        }
    }
}
