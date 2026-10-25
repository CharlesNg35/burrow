use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::net::IpAddr;
use std::sync::Arc;

pub use burrow_syntax::url_path::{on_prefix, path_matches};
use burrow_syntax::web::wildcard_of;

use crate::http::site::SiteRun;
use crate::{
    ClientAuth, Forwarding, ProxySettings, Site, SiteTls, TunnelMode, TunnelPort, TunnelSettings,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Passthrough(String),
    Tunnel { path: String },
    Http { routes: Vec<Route>, redirect: bool },
    Plain { routes: Vec<Route>, redirect: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub prefix: String,
    pub action: Action,
    pub app_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Tunnel,
    Service(String),
    Redirect { to: String, permanent: bool },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Table {
    hosts: HashMap<String, Kind>,
    plain_tunnel: Option<PlainTunnel>,
    sites: HashMap<String, Arc<SiteRun>>,
    client_auth: HashMap<String, ClientAuth>,
    pub forwarding: Forwarding,
    pub request_id: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlainTunnel {
    host: String,
    path: String,
    whole_name: bool,
}

fn route(prefix: &str, action: Action, app_path: Option<String>) -> Route {
    Route {
        prefix: prefix.to_string(),
        action,
        app_path,
    }
}

fn site_routes(site: &Site) -> Vec<Route> {
    let o = &site.options;
    let service = || Action::Service(site.service.clone());
    let mut routes: Vec<Route> = if o.paths.is_empty() {
        vec![route("/", service(), None)]
    } else {
        o.paths
            .iter()
            .map(|p| route(p, service(), o.app_path.clone()))
            .collect()
    };
    routes.extend(o.redirects.iter().map(|r| {
        let to = Action::Redirect {
            to: r.to.clone(),
            permanent: r.permanent,
        };
        route(&r.from, to, None)
    }));
    routes
}

fn empty_kind(site: &Site) -> Kind {
    match &site.tls {
        SiteTls::Passthrough => Kind::Passthrough(site.service.clone()),
        SiteTls::Https(_) => Kind::Http {
            routes: Vec::new(),
            redirect: site.https_redirect,
        },
        SiteTls::Plain => Kind::Plain {
            routes: Vec::new(),
            redirect: site.https_redirect,
        },
    }
}

fn merge(hosts: &mut HashMap<String, Kind>, host: &str, kind: &Kind, routes: &[Route]) {
    match hosts.entry(host.to_string()) {
        Entry::Vacant(v) => {
            let mut kind = kind.clone();
            if let Kind::Http { routes: r, .. } | Kind::Plain { routes: r, .. } = &mut kind {
                r.extend_from_slice(routes);
            }
            v.insert(kind);
        }
        Entry::Occupied(mut o) => {
            if let Kind::Http { routes: r, .. } | Kind::Plain { routes: r, .. } = o.get_mut() {
                r.extend_from_slice(routes);
            }
        }
    }
}

fn origin(site: &Site) -> Option<String> {
    let first = site.hosts.first()?;
    let scheme = if matches!(site.tls, SiteTls::Plain) {
        "http"
    } else {
        "https"
    };
    let host = if first.contains(':') {
        format!("[{first}]")
    } else {
        first.clone()
    };
    Some(format!("{scheme}://{host}"))
}

fn add_site(hosts: &mut HashMap<String, Kind>, site: &Site) {
    let kind = empty_kind(site);
    let routes = site_routes(site);
    for host in &site.hosts {
        merge(hosts, host, &kind, &routes);
    }
    if let Some(to) = origin(site).filter(|_| !site.options.redirect_from.is_empty()) {
        let away = [route(
            "/",
            Action::Redirect {
                to,
                permanent: true,
            },
            None,
        )];
        for name in &site.options.redirect_from {
            merge(hosts, name, &kind, &away);
        }
    }
}

impl Table {
    pub fn build(settings: &ProxySettings) -> Table {
        Table::build_after(settings, None)
    }

    pub fn build_after(settings: &ProxySettings, old: Option<&Table>) -> Table {
        let mut hosts = HashMap::new();
        let mut sites = HashMap::new();
        let mut client_auth = HashMap::new();
        for site in &settings.sites {
            add_site(&mut hosts, site);
            if matches!(site.tls, SiteTls::Passthrough) {
                continue;
            }
            let before = old.and_then(|t| t.site(&site.service)).map(Arc::as_ref);
            sites.insert(site.service.clone(), SiteRun::build_after(site, before));
            if let Some(c) = &site.options.client_auth {
                for host in &site.hosts {
                    client_auth.insert(host.clone(), c.clone());
                }
            }
        }
        let named = if settings.https.is_some() {
            Kind::Http {
                routes: Vec::new(),
                redirect: true,
            }
        } else {
            Kind::Plain {
                routes: Vec::new(),
                redirect: false,
            }
        };
        for r in &settings.redirect_names {
            let to = Action::Redirect {
                to: r.to.clone(),
                permanent: r.permanent,
            };
            merge(&mut hosts, &r.host, &named, &[route(&r.path, to, None)]);
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
            if let Kind::Http { routes, .. } | Kind::Plain { routes, .. } = kind {
                routes.sort_by_key(|r| std::cmp::Reverse(r.prefix.len()));
            }
        }
        Table {
            hosts,
            plain_tunnel,
            sites,
            client_auth,
            forwarding: settings.forwarding.clone(),
            request_id: settings.request_id,
        }
    }

    pub fn site(&self, service: &str) -> Option<&Arc<SiteRun>> {
        self.sites.get(service)
    }

    pub fn client_auth(&self, name: &str) -> Option<&ClientAuth> {
        if self.hosts.contains_key(name) {
            return self.client_auth.get(name);
        }
        self.client_auth.get(&wildcard_of(name)?)
    }

    pub fn limits(&self) -> impl Iterator<Item = &Arc<crate::auth::Limit>> {
        self.sites.values().filter_map(|s| s.limit.as_ref())
    }

    pub fn services_with<'a>(&'a self, key: &'a ClientAuth) -> impl Iterator<Item = &'a str> {
        self.sites
            .values()
            .filter(move |s| s.options.client_auth.as_ref() == Some(key))
            .map(|s| s.service.as_str())
    }

    pub fn client_auths(&self) -> impl Iterator<Item = &ClientAuth> {
        self.client_auth.values()
    }

    pub fn plain_tunnel(&self, host: &str, path: &str) -> Option<&str> {
        let t = self.plain_tunnel.as_ref()?;
        let here = t.host == host && (t.whole_name || path_matches(&t.path, path));
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
        (!matches!(kind, Kind::Plain { .. })).then_some((name, kind))
    }

    pub fn route<'a>(&'a self, host: &str, path: &str) -> Option<&'a Route> {
        match self.kind(host)? {
            Kind::Http { routes, .. } | Kind::Plain { routes, .. } => {
                routes.iter().find(|r| path_matches(&r.prefix, path))
            }
            _ => None,
        }
    }

    pub fn hosts(&self) -> impl Iterator<Item = (&String, &Kind)> {
        self.hosts.iter()
    }
}

fn add_tunnel(hosts: &mut HashMap<String, Kind>, t: &TunnelSettings) {
    let route = route(&t.path, Action::Tunnel, None);
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

#[cfg(test)]
mod tests;
