use burrow_config::ServerConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Server,
    Connections,
    Web,
    Monitoring,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Server, Tab::Connections, Tab::Web, Tab::Monitoring];

    pub fn key(self) -> &'static str {
        match self {
            Tab::Server => "server",
            Tab::Connections => "connections",
            Tab::Web => "proxy",
            Tab::Monitoring => "monitoring",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Server => "Server",
            Tab::Connections => "Connections",
            Tab::Web => "Web",
            Tab::Monitoring => "Monitoring",
        }
    }

    pub fn parse(key: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| t.key() == key)
    }
}

pub fn tab_of(field: &str) -> Tab {
    let is = |k: &str| field == k;
    if is(ServerConfig::WEB.key()) {
        Tab::Web
    } else if is(ServerConfig::METRICS.key()) || is(ServerConfig::NOTIFY.key()) {
        Tab::Monitoring
    } else if [
        ServerConfig::WEBSOCKET.key(),
        ServerConfig::HEARTBEAT.key(),
        ServerConfig::LIMITS.key(),
        ServerConfig::POOL.key(),
        ServerConfig::TCP.key(),
        ServerConfig::UDP.key(),
    ]
    .contains(&field)
    {
        Tab::Connections
    } else {
        Tab::Server
    }
}

pub fn problems_in(tab: Tab, paths: &[Vec<String>], sections: &[&str]) -> usize {
    paths
        .iter()
        .filter_map(|p| p.first())
        .filter(|k| !sections.contains(&k.as_str()) && tab_of(k) == tab)
        .count()
}

pub fn rank(field: &str) -> usize {
    [
        ServerConfig::WEBSOCKET.key(),
        ServerConfig::POOL.key(),
        ServerConfig::HEARTBEAT.key(),
        ServerConfig::TCP.key(),
        ServerConfig::UDP.key(),
        ServerConfig::LIMITS.key(),
    ]
    .iter()
    .position(|k| *k == field)
    .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_connections_tab_follows_the_ruled_order() {
        let mut keys = [
            ServerConfig::LIMITS.key(),
            ServerConfig::UDP.key(),
            ServerConfig::WEBSOCKET.key(),
            ServerConfig::HEARTBEAT.key(),
            ServerConfig::TCP.key(),
            ServerConfig::POOL.key(),
        ];
        keys.sort_by_key(|k| rank(k));
        assert_eq!(
            keys,
            [
                ServerConfig::WEBSOCKET.key(),
                ServerConfig::POOL.key(),
                ServerConfig::HEARTBEAT.key(),
                ServerConfig::TCP.key(),
                ServerConfig::UDP.key(),
                ServerConfig::LIMITS.key(),
            ]
        );
    }

    #[test]
    fn each_server_field_has_its_settings_tab() {
        assert_eq!(tab_of(ServerConfig::LISTEN.key()), Tab::Server);
        assert_eq!(tab_of(ServerConfig::PUBLIC_ADDRESS.key()), Tab::Server);
        assert_eq!(tab_of(ServerConfig::INSECURE_PLAINTEXT.key()), Tab::Server);
        assert_eq!(tab_of(ServerConfig::WEBSOCKET.key()), Tab::Connections);
        assert_eq!(tab_of(ServerConfig::POOL.key()), Tab::Connections);
        assert_eq!(tab_of(ServerConfig::LIMITS.key()), Tab::Connections);
        assert_eq!(tab_of(ServerConfig::WEB.key()), Tab::Web);
        assert_eq!(tab_of(ServerConfig::METRICS.key()), Tab::Monitoring);
        assert_eq!(tab_of(ServerConfig::NOTIFY.key()), Tab::Monitoring);
        for t in Tab::ALL {
            assert_eq!(Tab::parse(t.key()), Some(t));
        }
    }

    #[test]
    fn each_tab_counts_the_problems_of_its_own_settings() {
        let p = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let paths = [
            p(&["web", "http"]),
            p(&["pool", "min_idle"]),
            p(&["services", "web", "bind"]),
            p(&["listen"]),
        ];
        let sections = ["services", "clients", "visitors"];
        assert_eq!(problems_in(Tab::Web, &paths, &sections), 1);
        assert_eq!(problems_in(Tab::Connections, &paths, &sections), 1);
        assert_eq!(problems_in(Tab::Server, &paths, &sections), 1);
        assert_eq!(problems_in(Tab::Monitoring, &paths, &sections), 0);
    }
}
