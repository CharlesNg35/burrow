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
    } else if is(ServerConfig::METRICS.key()) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        for t in Tab::ALL {
            assert_eq!(Tab::parse(t.key()), Some(t));
        }
    }
}
