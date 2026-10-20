use crate::config::check::parse_host_port;
use crate::config::settings::ServerSettings;
use crate::protocol::{GrantedClient, GrantedService, Grants};
use burrow_config::Protocol;

pub fn for_visitor(settings: &ServerSettings, visitor: &str) -> Option<Grants> {
    let auth = settings.visitors.get(visitor)?;
    let services = settings
        .services
        .iter()
        .filter(|(_, s)| s.protocol == Protocol::Tcp && s.visitors.iter().any(|v| v == visitor))
        .map(|(name, s)| GrantedService {
            name: name.clone(),
            port: parse_host_port(&s.target).map_or(0, |(_, p)| p),
            ports: s.ports.max(1),
        })
        .collect();
    let clients = auth
        .reach
        .iter()
        .map(|(name, rules)| GrantedClient {
            name: name.clone(),
            reach: rules.rules().unwrap_or_default(),
            allow: None,
        })
        .collect();
    Some(Grants { services, clients })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Format, Loaded, Role, load_str};

    #[test]
    fn a_visitor_sees_every_service_it_is_granted_wherever_it_asks_from() {
        let s = || crate::secret::Secret::generate().encode();
        let yaml = format!(
            "listen: 7835\nclients:\n  home: {{ secret: {} }}\n  office: {{ secret: {} }}\nvisitors:\n  laptop: {{ secret: {}, reach: {{ home: any }} }}\n  other: {{ secret: {} }}\nservices:\n  db: {{ client: home, target: 5432, visitors: [laptop] }}\n  lan: {{ client: home, target: 22, visitors: [laptop], allow_from: [10.0.0.0/8] }}\n  theirs: {{ client: office, target: 80, visitors: [other] }}\n  udp: {{ client: home, protocol: udp, bind: 5353, target: 53 }}\n",
            s(),
            s(),
            s(),
            s()
        );
        let nothing = |_: &str| None;
        let Some(Loaded::Server(settings, _)) = load_str(
            "server.yaml",
            &yaml,
            Format::Yaml,
            Some(Role::Server),
            &nothing,
        )
        .loaded
        else {
            panic!("server config loads")
        };
        let names = |g: &Grants| {
            g.services
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
        };
        let laptop = for_visitor(&settings, "laptop").unwrap();
        assert_eq!(names(&laptop), vec!["db", "lan"]);
        assert_eq!(laptop.clients.len(), 1);
        assert_eq!(laptop.clients[0].name, "home");
        assert!(for_visitor(&settings, "nobody").is_none());
        assert!(for_visitor(&settings, "other").unwrap().clients.is_empty());
    }
}
