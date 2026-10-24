use std::collections::HashMap;
use std::net::IpAddr;

use burrow_syntax::web::wildcard_of;

use crate::{ProxySettings, SiteTls, TunnelMode, TunnelPort, TunnelSettings};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Passthrough(String),
    Tunnel { path: String },
    Http { routes: Vec<Route>, redirect: bool },
    Plain(Vec<Route>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub prefix: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Tunnel,
    Service(String),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Table {
    hosts: HashMap<String, Kind>,
    plain_tunnel: Option<PlainTunnel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlainTunnel {
    host: String,
    path: String,
    whole_name: bool,
}

fn whole(service: &str) -> Vec<Route> {
    vec![Route {
        prefix: "/".into(),
        action: Action::Service(service.to_string()),
    }]
}

impl Table {
    pub fn build(settings: &ProxySettings) -> Table {
        let mut hosts = HashMap::new();
        for site in &settings.sites {
            let kind = match &site.tls {
                SiteTls::Passthrough => Kind::Passthrough(site.service.clone()),
                SiteTls::Https(_) => Kind::Http {
                    routes: whole(&site.service),
                    redirect: site.https_redirect,
                },
                SiteTls::Plain => Kind::Plain(whole(&site.service)),
            };
            for host in &site.hosts {
                hosts.entry(host.clone()).or_insert_with(|| kind.clone());
            }
        }
        let plain_tunnel = match &settings.tunnel {
            Some(t) if t.port == TunnelPort::Http => Some(PlainTunnel {
                host: t.host.clone(),
                path: t.path.clone(),
                whole_name: t.mode == TunnelMode::OwnName,
            }),
            Some(t) => {
                add_tunnel(&mut hosts, t);
                None
            }
            None => None,
        };
        for kind in hosts.values_mut() {
            if let Kind::Http { routes, .. } | Kind::Plain(routes) = kind {
                routes.sort_by_key(|r| std::cmp::Reverse(r.prefix.len()));
            }
        }
        Table {
            hosts,
            plain_tunnel,
        }
    }

    pub fn plain_tunnel(&self, host: &str, path: &str) -> Option<&str> {
        let t = self.plain_tunnel.as_ref()?;
        let here = t.host == host && (t.whole_name || on_prefix(&t.path, path));
        here.then_some(t.path.as_str())
    }

    pub fn kind(&self, name: &str) -> Option<&Kind> {
        self.hosts
            .get(name)
            .or_else(|| wildcard_of(name).and_then(|w| self.hosts.get(&w)))
    }

    pub fn pick(&self, sni: Option<&str>, local: IpAddr) -> Option<(String, &Kind)> {
        let (name, kind) = match sni {
            Some(name) => (name.to_string(), self.kind(name)?),
            None => {
                let ip = local.to_canonical().to_string();
                let kind = self.hosts.get(&ip)?;
                (ip, kind)
            }
        };
        (!matches!(kind, Kind::Plain(_))).then_some((name, kind))
    }

    pub fn route<'a>(&'a self, host: &str, path: &str) -> Option<&'a Route> {
        match self.kind(host)? {
            Kind::Http { routes, .. } | Kind::Plain(routes) => {
                routes.iter().find(|r| on_prefix(&r.prefix, path))
            }
            _ => None,
        }
    }

    pub fn hosts(&self) -> impl Iterator<Item = (&String, &Kind)> {
        self.hosts.iter()
    }
}

fn add_tunnel(hosts: &mut HashMap<String, Kind>, t: &TunnelSettings) {
    let route = Route {
        prefix: t.path.clone(),
        action: Action::Tunnel,
    };
    match (t.mode, hosts.get_mut(&t.host)) {
        (TunnelMode::ByPath, Some(Kind::Http { routes, .. })) => routes.insert(0, route),
        (_, Some(_)) => {}
        (TunnelMode::OwnName, None) => {
            hosts.insert(
                t.host.clone(),
                Kind::Tunnel {
                    path: t.path.clone(),
                },
            );
        }
        (TunnelMode::ByPath, None) => {
            hosts.insert(
                t.host.clone(),
                Kind::Http {
                    routes: vec![route],
                    redirect: true,
                },
            );
        }
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
            Site::passthrough("nas", vec!["nas.example.com".into(), "203.0.113.10".into()]),
            Site::passthrough("apps", vec!["*.apps.example.com".into()]),
            Site::passthrough("exact", vec!["one.apps.example.com".into()]),
            Site {
                tls: SiteTls::Plain,
                ..Site::passthrough("plain", vec!["plain.example.com".into()])
            },
        ];
        s.tunnel = tunnel.map(|(host, mode)| TunnelSettings {
            host: host.into(),
            path: "/burrow".into(),
            mode,
            port: crate::TunnelPort::Https,
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
        let local: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(t.pick(Some("plain.example.com"), local).is_none());
        assert!(matches!(t.kind("plain.example.com"), Some(Kind::Plain(_))));
    }

    fn served(t: &Table, host: &str, path: &str) -> Option<Action> {
        t.route(host, path).map(|r| r.action.clone())
    }

    #[test]
    fn a_terminated_site_owns_every_path_and_keeps_its_redirect_choice() {
        let mut s = settings(None);
        s.sites.push(Site {
            tls: SiteTls::Https(crate::CertSource::Auto),
            ..Site::passthrough("blog", vec!["blog.example.com".into(), "nas".into()])
        });
        s.sites.push(Site {
            tls: SiteTls::Https(crate::CertSource::Auto),
            https_redirect: false,
            ..Site::passthrough("open", vec!["open.example.com".into()])
        });
        let t = Table::build(&s);
        let blog = Some(Action::Service("blog".into()));
        assert_eq!(served(&t, "blog.example.com", "/"), blog);
        assert_eq!(served(&t, "blog.example.com", "/a/b"), blog);
        assert_eq!(served(&t, "nas", "/"), blog);
        assert_eq!(
            served(&t, "plain.example.com", "/x"),
            Some(Action::Service("plain".into()))
        );
        assert_eq!(served(&t, "nas.example.com", "/"), None);
        assert!(matches!(
            t.kind("blog.example.com"),
            Some(Kind::Http { redirect: true, .. })
        ));
        assert!(matches!(
            t.kind("open.example.com"),
            Some(Kind::Http {
                redirect: false,
                ..
            })
        ));
    }

    #[test]
    fn the_tunnel_by_path_shares_a_terminated_sites_name_and_wins_its_prefix() {
        let mut s = settings(Some(("blog.example.com", TunnelMode::ByPath)));
        s.sites.push(Site {
            tls: SiteTls::Https(crate::CertSource::Auto),
            ..Site::passthrough("blog", vec!["blog.example.com".into()])
        });
        let t = Table::build(&s);
        assert_eq!(
            served(&t, "blog.example.com", "/burrow"),
            Some(Action::Tunnel)
        );
        assert_eq!(
            served(&t, "blog.example.com", "/burrow/x"),
            Some(Action::Tunnel)
        );
        assert_eq!(
            served(&t, "blog.example.com", "/burrowx"),
            Some(Action::Service("blog".into()))
        );
        let on_passthrough = Table::build(&settings(Some(("nas.example.com", TunnelMode::ByPath))));
        assert_eq!(
            on_passthrough.kind("nas.example.com"),
            Some(&Kind::Passthrough("nas".into()))
        );
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
