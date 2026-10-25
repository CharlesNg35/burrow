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
    assert!(matches!(
        t.kind("plain.example.com"),
        Some(Kind::Plain { .. })
    ));
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

fn https(service: &str, hosts: &[&str], options: crate::SiteOptions) -> Site {
    Site {
        tls: SiteTls::Https(crate::CertSource::Auto),
        options,
        ..Site::passthrough(service, hosts.iter().map(|h| h.to_string()).collect())
    }
}

fn paths(list: &[&str], app_path: Option<&str>) -> crate::SiteOptions {
    crate::SiteOptions {
        paths: list.iter().map(|p| p.to_string()).collect(),
        app_path: app_path.map(str::to_string),
        ..crate::SiteOptions::default()
    }
}

fn redirect(to: &str, permanent: bool) -> Option<Action> {
    Some(Action::Redirect {
        to: to.into(),
        permanent,
    })
}

#[test]
fn services_share_a_host_by_paths_and_the_longest_prefix_wins() {
    let mut s = ProxySettings::new("[::]:443".parse().unwrap());
    s.sites = vec![
        https("blog", &["example.com"], crate::SiteOptions::default()),
        https("api", &["example.com"], paths(&["/api"], Some("/"))),
        https("v2", &["example.com"], paths(&["/api/v2"], None)),
        https("admin", &["example.com"], paths(&["/wp-admin"], None)),
    ];
    let t = Table::build(&s);
    let svc = |n: &str| Some(Action::Service(n.into()));
    assert_eq!(served(&t, "example.com", "/"), svc("blog"));
    assert_eq!(served(&t, "example.com", "/apix"), svc("blog"));
    assert_eq!(served(&t, "example.com", "/api"), svc("api"));
    assert_eq!(served(&t, "example.com", "/api/users"), svc("api"));
    assert_eq!(served(&t, "example.com", "/api/v2/x"), svc("v2"));
    assert_eq!(served(&t, "example.com", "/wp-admin/x"), svc("admin"));
    assert_eq!(served(&t, "example.com", "/wp-admin..%2Fx"), svc("blog"));
    let api = t.route("example.com", "/api/x").unwrap();
    assert_eq!(api.app_path.as_deref(), Some("/"));
    assert_eq!(api.prefix, "/api");
    assert!(t.site("api").is_some() && t.site("nope").is_none());
}

#[test]
fn a_host_with_only_paths_answers_nothing_else() {
    let mut s = ProxySettings::new("[::]:443".parse().unwrap());
    s.sites = vec![https("api", &["api.example.com"], paths(&["/v1"], None))];
    let t = Table::build(&s);
    assert!(t.route("api.example.com", "/v1/x").is_some());
    assert!(t.route("api.example.com", "/").is_none());
}

#[test]
fn redirects_redirect_from_and_redirect_names_become_routes() {
    let mut options = paths(&["/api"], Some("/v1"));
    options.redirects = vec![
        crate::Redirect {
            from: "/old".into(),
            to: "/new".into(),
            permanent: false,
        },
        crate::Redirect {
            from: "/api/old".into(),
            to: "https://docs.example.com".into(),
            permanent: true,
        },
    ];
    options.redirect_from = vec!["www.example.com".into()];
    let mut s = ProxySettings::new("[::]:443".parse().unwrap());
    s.sites = vec![
        https("app", &["example.com", "app.example.com"], options),
        https("blog", &["example.com"], crate::SiteOptions::default()),
    ];
    s.redirect_names = vec![
        crate::NameRedirect {
            host: "old.example.com".into(),
            path: "/".into(),
            to: "https://example.com".into(),
            permanent: false,
        },
        crate::NameRedirect {
            host: "example.com".into(),
            path: "/shop".into(),
            to: "https://shop.example.net".into(),
            permanent: true,
        },
    ];
    let t = Table::build(&s);
    assert_eq!(served(&t, "example.com", "/old/x"), redirect("/new", false));
    assert_eq!(
        served(&t, "example.com", "/older"),
        Some(Action::Service("blog".into()))
    );
    assert_eq!(
        served(&t, "example.com", "/api/old"),
        redirect("https://docs.example.com", true)
    );
    assert_eq!(
        served(&t, "example.com", "/api/users"),
        Some(Action::Service("app".into()))
    );
    assert_eq!(
        served(&t, "www.example.com", "/any/path"),
        redirect("https://example.com", true)
    );
    assert!(matches!(
        t.kind("www.example.com"),
        Some(Kind::Http { redirect: true, .. })
    ));
    assert_eq!(
        served(&t, "old.example.com", "/x"),
        redirect("https://example.com", false)
    );
    assert_eq!(
        served(&t, "example.com", "/shop/cart"),
        redirect("https://shop.example.net", true)
    );
    let plain = ProxySettings {
        redirect_names: s.redirect_names.clone(),
        ..ProxySettings::default()
    };
    assert!(matches!(
        Table::build(&plain).kind("old.example.com"),
        Some(Kind::Plain { .. })
    ));
}
