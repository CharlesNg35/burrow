use burrow_admin_proto::codes::{CLOSES_PORT, REMOVES_VISITORS, STOPS_WEBSITE};
use burrow_admin_proto::plan::{Impact, Level};
use burrow_config::{ServiceEntry, WebSite as Site};

use crate::config::settings::{ServerSettings, Service, WebSite, WebTls};

fn port_number(svc: &Service) -> Option<String> {
    let b = svc.bind?;
    Some(match svc.ports {
        0 | 1 => b.port().to_string(),
        n => format!("{}-{}", b.port(), b.port().saturating_add(n - 1)),
    })
}

fn closes_port(old: &Service) -> Option<String> {
    let port = port_number(old)?;
    let shown = old.shown_bind().unwrap_or_default();
    Some(format!("closes port {port} (removes bind: {shown})"))
}

fn users(n: usize) -> String {
    format!("{n} {}", crate::config::diagnostics::plural(n, "user"))
}

fn site_settings(site: &WebSite) -> Vec<String> {
    let o = &site.options;
    let mut out = Vec::new();
    match site.tls {
        WebTls::Passthrough => out.push("HTTPS (the app does its own)".to_string()),
        WebTls::Off => out.push("plain HTTP".to_string()),
        _ => out.push("HTTPS".to_string()),
    }
    if !o.basic_auth.is_empty() {
        out.push(format!(
            "{} ({})",
            Site::BASIC_AUTH.key(),
            users(o.basic_auth.len())
        ));
    }
    if o.forward_auth.is_some() {
        out.push(Site::FORWARD_AUTH.key().into());
    }
    if o.client_auth.is_some() {
        out.push(Site::CLIENT_AUTH.key().into());
    }
    if !o.redirects.is_empty() || !o.redirect_from.is_empty() {
        out.push(Site::REDIRECTS.key().into());
    }
    if !o.paths.is_empty() {
        out.push(Site::PATHS.key().into());
    }
    if !o.request_headers.is_empty() || !o.response_headers.is_empty() {
        out.push("headers".into());
    }
    out
}

fn stops_website(name: &str, site: &WebSite) -> String {
    let first = site
        .hosts
        .first()
        .cloned()
        .unwrap_or_else(|| name.to_string());
    format!(
        "stops website {first} and removes its settings: {}",
        site_settings(site).join(", ")
    )
}

fn removes_visitors(name: &str, gone: &[&String]) -> String {
    let list: Vec<&str> = gone.iter().map(|s| s.as_str()).collect();
    let (noun, pronoun) = if gone.len() == 1 {
        ("visitor", "it")
    } else {
        (ServiceEntry::VISITORS.key(), "they")
    };
    format!(
        "removes {noun} {}: {pronoun} can no longer reach {name} with burrow visit",
        list.join(", ")
    )
}

pub struct Seen<'a> {
    pub renamed: &'a dyn Fn(&str) -> bool,
    pub open: &'a dyn Fn(&str) -> u64,
}

pub fn impacts(old: &ServerSettings, new: &ServerSettings, seen: &Seen) -> Vec<Impact> {
    let mut out = Vec::new();
    let impact = |level, code: &str, message, confirm| Impact {
        level,
        code: code.into(),
        message,
        confirm,
    };
    for (name, svc) in &old.services {
        let Some(next) = new.services.get(name).filter(|_| !(seen.renamed)(name)) else {
            continue;
        };
        if next.bind.is_none()
            && let Some(mut m) = closes_port(svc)
        {
            let open = (seen.open)(name);
            let (level, confirm) = if open > 0 {
                m.push_str(&format!(
                    "; closes {open} open {}",
                    crate::config::diagnostics::plural(open as usize, "connection")
                ));
                (Level::Danger, Some(name.clone()))
            } else {
                (Level::Warning, None)
            };
            out.push(impact(level, CLOSES_PORT, m, confirm));
        }
        let gone: Vec<&String> = svc
            .visitors
            .iter()
            .filter(|v| !next.visitors.contains(v))
            .collect();
        if !gone.is_empty() {
            out.push(impact(
                Level::Warning,
                REMOVES_VISITORS,
                removes_visitors(name, &gone),
                None,
            ));
        }
        let site = old.web.as_ref().and_then(|w| w.sites.get(name));
        let still = new.web.as_ref().map(|w| w.sites.contains_key(name));
        if let (Some(site), Some(false)) = (site, still) {
            out.push(impact(
                Level::Warning,
                STOPS_WEBSITE,
                stops_website(name, site),
                Some(name.clone()),
            ));
        }
    }
    out
}
