use std::collections::BTreeMap;

use super::{HostLine, automatic, host};
use crate::config::settings::{TargetTls, WebSettings, WebSite, WebTls};
use burrow_admin_proto::web::{NOT_FOUND, RouteLine};
use burrow_syntax::address::url_host;

fn kind(permanent: bool) -> &'static str {
    if permanent { "permanent" } else { "temporary" }
}

fn extras(site: &WebSite, path: &str, trusted: bool) -> String {
    let o = &site.options;
    let mut parts = Vec::new();
    if let Some(app) = o.app_path.as_deref().filter(|a| *a != path) {
        parts.push(format!("app sees {app}, X-Forwarded-Prefix {path}"));
    }
    match &o.target_tls {
        TargetTls::Off => {}
        TargetTls::PublicRoots => parts.push("HTTPS to the app".to_string()),
        TargetTls::Ca(ca) => parts.push(format!("HTTPS to the app, checked with {}", ca.display())),
        TargetTls::Unchecked => parts.push("HTTPS to the app, not checked".to_string()),
    }
    if o.target_http2 {
        parts.push("HTTP/2 to the app".into());
    }
    if o.maintenance {
        parts.push("maintenance page".into());
    }
    if site.tls == WebTls::Off && site.https_redirect && trusted {
        parts.push("http:// at the proxy redirects to https:// by X-Forwarded-Proto".into());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join("; "))
    }
}

fn site_routes(service: &str, site: &WebSite, web: &WebSettings) -> Vec<RouteLine> {
    let trusted = !web.trusted_proxies.is_empty();
    let mut out: Vec<RouteLine> = site
        .options
        .redirects
        .iter()
        .map(|r| RouteLine {
            path: r.from.clone(),
            to: format!(
                "redirect {} ({})",
                web.written.shown(&r.to),
                kind(r.permanent)
            ),
            service: String::new(),
        })
        .collect();
    let paths: Vec<&str> = if site.options.paths.is_empty() {
        vec!["/"]
    } else {
        site.options.paths.iter().map(String::as_str).collect()
    };
    for p in paths {
        out.push(RouteLine {
            path: p.to_string(),
            to: format!("service {service}{}", extras(site, p, trusted)),
            service: service.to_string(),
        });
    }
    out
}

fn sorted(mut routes: Vec<RouteLine>) -> Vec<RouteLine> {
    routes.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.path.cmp(&b.path)));
    if !routes.iter().any(|r| r.path == "/") {
        routes.push(RouteLine {
            path: NOT_FOUND.into(),
            to: "404".into(),
            service: String::new(),
        });
    }
    routes
}

pub(super) fn hosts(web: &WebSettings) -> Vec<HostLine> {
    let mut lines: BTreeMap<String, (HostLine, Vec<RouteLine>, usize)> = BTreeMap::new();
    for (service, site) in &web.sites {
        for name in &site.hosts {
            let entry = lines
                .entry(name.clone())
                .or_insert_with(|| (host(web, service, site, name), Vec::new(), 0));
            if site.tls != WebTls::Passthrough {
                entry.1.extend(site_routes(service, site, web));
            }
            entry.2 += 1;
        }
        if site.tls == WebTls::Passthrough {
            continue;
        }
        let Some(first) = site.hosts.first() else {
            continue;
        };
        for name in &site.options.redirect_from {
            let line = host(web, service, site, name);
            let scheme = if site.tls == WebTls::Off {
                "http"
            } else {
                "https"
            };
            let route = RouteLine {
                path: "/".into(),
                to: format!("redirect {scheme}://{} (permanent)", url_host(first)),
                service: String::new(),
            };
            lines.insert(name.clone(), (line, vec![route], 2));
        }
    }
    for r in &web.redirect_names {
        let route = RouteLine {
            path: r.path.clone(),
            to: format!(
                "redirect {} ({})",
                web.written.shown(&r.to),
                kind(r.permanent)
            ),
            service: String::new(),
        };
        let entry = lines.entry(r.host.clone()).or_insert_with(|| {
            let (certificate, check) = if web.https.is_some() {
                automatic(web, &r.host, None)
            } else {
                (super::PLAIN.to_string(), String::new())
            };
            let line = HostLine {
                name: r.host.clone(),
                kind: if web.https.is_some() {
                    "HTTPS"
                } else {
                    super::PLAIN
                }
                .into(),
                certificate,
                check,
                ..HostLine::default()
            };
            (line, Vec::new(), 2)
        });
        entry.1.push(route);
        entry.2 += 1;
    }
    lines
        .into_values()
        .map(|(mut line, routes, users)| {
            let simple = users == 1
                && routes.len() == 1
                && routes[0].path == "/"
                && routes[0].to == format!("service {}", line.service);
            if !simple && !routes.is_empty() {
                line.routes = sorted(routes);
                line.service = String::new();
            }
            line
        })
        .collect()
}
