use crate::config::check::parse_host_port;
use crate::config::model::Protocol;
use crate::config::settings::ServerSettings;
use crate::protocol::{GrantedClient, GrantedService, Grants};

pub fn for_visitor(
    settings: &ServerSettings,
    visitor: &str,
    from: Option<std::net::IpAddr>,
) -> Option<Grants> {
    let auth = settings.visitors.get(visitor)?;
    let services = settings
        .services
        .iter()
        .filter(|(_, s)| s.protocol == Protocol::Tcp && s.visitors.iter().any(|v| v == visitor))
        .filter(|(_, s)| from.is_none_or(|ip| s.sources.permits(ip)))
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
    fn a_visitor_sees_only_what_it_may_use_from_where_it_is() {
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
        let local = for_visitor(&settings, "laptop", Some("127.0.0.1".parse().unwrap())).unwrap();
        assert_eq!(names(&local), vec!["db"]);
        assert_eq!(local.clients.len(), 1);
        assert_eq!(local.clients[0].name, "home");
        let inside = for_visitor(&settings, "laptop", Some("10.1.2.3".parse().unwrap())).unwrap();
        assert_eq!(names(&inside), vec!["db", "lan"]);
        assert_eq!(
            names(&for_visitor(&settings, "laptop", None).unwrap()),
            vec!["db", "lan"]
        );
        assert!(for_visitor(&settings, "nobody", None).is_none());
        assert!(
            for_visitor(&settings, "other", None)
                .unwrap()
                .clients
                .is_empty()
        );
    }
}
