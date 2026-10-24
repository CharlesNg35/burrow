use std::collections::HashMap;
use std::net::IpAddr;

use burrow_syntax::web::wildcard_of;

use crate::{ProxySettings, TunnelMode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Passthrough(String),
    Tunnel { path: String },
    Http(Vec<Route>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub prefix: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Tunnel,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Table {
    hosts: HashMap<String, Kind>,
}

impl Table {
    pub fn build(settings: &ProxySettings) -> Table {
        let mut hosts = HashMap::new();
        for site in &settings.sites {
            for host in &site.hosts {
                hosts
                    .entry(host.clone())
                    .or_insert_with(|| Kind::Passthrough(site.service.clone()));
            }
        }
        if let Some(t) = &settings.tunnel {
            let kind = match t.mode {
                TunnelMode::OwnName => Kind::Tunnel {
                    path: t.path.clone(),
                },
                TunnelMode::ByPath => Kind::Http(vec![Route {
                    prefix: t.path.clone(),
                    action: Action::Tunnel,
                }]),
            };
            hosts.entry(t.host.clone()).or_insert(kind);
        }
        Table { hosts }
    }

    pub fn kind(&self, name: &str) -> Option<&Kind> {
        self.hosts
            .get(name)
            .or_else(|| wildcard_of(name).and_then(|w| self.hosts.get(&w)))
    }

    pub fn pick(&self, sni: Option<&str>, local: IpAddr) -> Option<(String, &Kind)> {
        match sni {
            Some(name) => self.kind(name).map(|k| (name.to_string(), k)),
            None => {
                let ip = local.to_canonical().to_string();
                self.hosts.get(&ip).map(|k| (ip, k))
            }
        }
    }

    pub fn route<'a>(&'a self, host: &str, path: &str) -> Option<&'a Route> {
        match self.kind(host)? {
            Kind::Http(routes) => routes.iter().find(|r| on_prefix(&r.prefix, path)),
            _ => None,
        }
    }

    pub fn hosts(&self) -> impl Iterator<Item = (&String, &Kind)> {
        self.hosts.iter()
    }
}

pub fn on_prefix(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Site, TunnelSettings};

    fn settings(tunnel: Option<(&str, TunnelMode)>) -> ProxySettings {
        let mut s = ProxySettings::new("[::]:443".parse().unwrap());
        s.sites = vec![
            Site {
                service: "nas".into(),
                hosts: vec!["nas.example.com".into(), "203.0.113.10".into()],
            },
            Site {
                service: "apps".into(),
                hosts: vec!["*.apps.example.com".into()],
            },
            Site {
                service: "exact".into(),
                hosts: vec!["one.apps.example.com".into()],
            },
        ];
        s.tunnel = tunnel.map(|(host, mode)| TunnelSettings {
            host: host.into(),
            path: "/burrow".into(),
            mode,
            tls: None,
        });
        s
    }

    #[test]
    fn exact_names_win_over_a_wildcard_of_one_label() {
        let t = Table::build(&settings(None));
        let pass = |s: &str| Some(Kind::Passthrough(s.into()));
        assert_eq!(t.kind("nas.example.com").cloned(), pass("nas"));
        assert_eq!(t.kind("x.apps.example.com").cloned(), pass("apps"));
        assert_eq!(t.kind("one.apps.example.com").cloned(), pass("exact"));
        assert_eq!(t.kind("a.b.apps.example.com"), None);
        assert_eq!(t.kind("apps.example.com"), None);
        assert_eq!(t.kind("example.com"), None);
        assert_eq!(t.kind("unknown.example.org"), None);
    }

    #[test]
    fn a_client_without_a_name_reaches_the_site_of_the_address_it_dialled() {
        let t = Table::build(&settings(None));
        let local: IpAddr = "203.0.113.10".parse().unwrap();
        let (name, kind) = t.pick(None, local).unwrap();
        assert_eq!(name, "203.0.113.10");
        assert_eq!(kind, &Kind::Passthrough("nas".into()));
        let mapped: IpAddr = "::ffff:203.0.113.10".parse().unwrap();
        assert!(t.pick(None, mapped).is_some());
        assert!(t.pick(None, "203.0.113.11".parse().unwrap()).is_none());
        assert!(t.pick(Some("203.0.113.99"), local).is_none());
    }

    #[test]
    fn the_tunnel_owns_its_name_or_a_path_on_it() {
        let own = Table::build(&settings(Some(("tunnel.example.com", TunnelMode::OwnName))));
        assert_eq!(
            own.kind("tunnel.example.com"),
            Some(&Kind::Tunnel {
                path: "/burrow".into()
            })
        );
        let by_path = Table::build(&settings(Some(("site.example.com", TunnelMode::ByPath))));
        let route = by_path.route("site.example.com", "/burrow").unwrap();
        assert_eq!(route.action, Action::Tunnel);
        assert!(by_path.route("site.example.com", "/burrow/x").is_some());
        assert!(by_path.route("site.example.com", "/burrowx").is_none());
        assert!(by_path.route("site.example.com", "/").is_none());
        assert!(by_path.route("nas.example.com", "/burrow").is_none());
    }

    #[test]
    fn prefixes_match_on_segment_boundaries() {
        assert!(on_prefix("/", "/anything"));
        assert!(on_prefix("/api", "/api"));
        assert!(on_prefix("/api", "/api/"));
        assert!(on_prefix("/api", "/api/x"));
        assert!(!on_prefix("/api", "/apix"));
        assert!(!on_prefix("/api", "/"));
    }
}
