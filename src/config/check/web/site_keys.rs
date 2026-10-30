use std::collections::BTreeMap;
use std::path::Path;

use super::{Served, Spans};
use crate::base::failure::file_reason;
use crate::config::diagnostics::Diagnostic;
use burrow_config::{Protocol, ServerConfig, ServiceWeb, Target, TargetTls, WebSite};
use burrow_syntax::url_path::{route_key, route_path};
use burrow_syntax::web::pages::ERROR_PAGES as PAGES;
use burrow_syntax::web::{SingleLabel, host_name, is_ip, is_wildcard};

pub(super) struct At<'a> {
    pub spans: &'a Spans,
    pub base: Vec<String>,
}

impl At<'_> {
    pub fn path(&self, rest: &[&str]) -> Vec<String> {
        let mut p = self.base.clone();
        p.extend(rest.iter().map(|s| s.to_string()));
        p
    }

    pub fn name(&self, rest: &[&str]) -> String {
        self.path(rest).join(".")
    }

    pub fn value(&self, rest: &[&str]) -> crate::config::tree::Span {
        let p = self.path(rest);
        let p: Vec<&str> = p.iter().map(String::as_str).collect();
        self.spans.value(&p)
    }

    pub fn key(&self, rest: &[&str]) -> crate::config::tree::Span {
        let p = self.path(rest);
        let p: Vec<&str> = p.iter().map(String::as_str).collect();
        self.spans.key(&p)
    }
}

pub(super) fn check_site_keys(
    cfg: &ServerConfig,
    https: bool,
    base_dir: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let Some(top) = cfg.web.as_ref() else {
        return;
    };
    let at = At {
        spans,
        base: vec!["web".into()],
    };
    shared(
        &Shared {
            max_body_size: top.max_body_size.as_deref(),
            request_headers: &top.request_headers,
            response_headers: &top.response_headers,
            error_pages: top.error_pages.as_deref(),
        },
        &at,
        base_dir,
        out,
    );
    for (name, svc) in &cfg.services {
        let Some(ServiceWeb::Site(site)) = svc.web.as_ref() else {
            continue;
        };
        if svc.protocol != Protocol::Tcp {
            continue;
        }
        let at = At {
            spans,
            base: vec!["services".into(), name.clone(), "web".into()],
        };
        let served = Served::of(
            svc.web.as_ref().unwrap_or(&ServiceWeb::Hosts(Vec::new())),
            https,
        );
        if served == Served::Passthrough {
            passthrough(site, &at, out);
            continue;
        }
        if site.target_http2 {
            grpc_notes(name, served, top.http2 == Some(false), &at, out);
        }
        if served == Served::Plain && site.hsts == Some(true) {
            let off = matches!(
                site.tls,
                Some(burrow_config::SiteTls::Mode(burrow_config::TlsMode::Off))
            );
            let (code, message) = if off {
                (
                    "plain-tls-key",
                    format!(
                        "services.{name}.web.hsts: a site with tls: off has no HTTPS; remove hsts"
                    ),
                )
            } else {
                (
                    super::choices::NEEDS_HTTPS,
                    super::choices::needs_https(name, "hsts", "hsts"),
                )
            };
            out.push(
                Diagnostic::error(code, message)
                    .at(at.value(&["hsts"]))
                    .help("HSTS is sent only on HTTPS answers"),
            );
        }
        paths(site, &at, out);
        redirects(&site.redirects, &at, out);
        redirect_from(site, &at, out);
        target_tls(name, site, &svc.target, &at, base_dir, out);
        shared(
            &Shared {
                max_body_size: site.max_body_size.as_deref(),
                request_headers: &site.request_headers,
                response_headers: &site.response_headers,
                error_pages: site.error_pages.as_deref(),
            },
            &at,
            base_dir,
            out,
        );
    }
}

fn passthrough(site: &WebSite, at: &At<'_>, out: &mut Vec<Diagnostic>) {
    let set: [(&str, bool); 18] = [
        ("paths", !site.paths.is_empty()),
        ("basic_auth", !site.basic_auth.is_empty()),
        ("forward_auth", site.forward_auth.is_some()),
        ("rate_limit", site.rate_limit.is_some()),
        ("client_auth", site.client_auth.is_some()),
        ("target_http2", site.target_http2),
        ("app_path", site.app_path.is_some()),
        ("redirect_from", !site.redirect_from.is_empty()),
        ("redirects", !site.redirects.is_empty()),
        ("maintenance", site.maintenance),
        ("target_tls", site.target_tls.is_some()),
        ("hsts", site.hsts.is_some()),
        ("compress", site.compress.is_some()),
        ("max_body_size", site.max_body_size.is_some()),
        (
            "response_header_timeout_secs",
            site.response_header_timeout_secs.is_some(),
        ),
        ("request_headers", !site.request_headers.is_empty()),
        ("response_headers", !site.response_headers.is_empty()),
        ("error_pages", site.error_pages.is_some()),
    ];
    for (key, _) in set.iter().filter(|(_, on)| *on) {
        out.push(
            Diagnostic::error(
                "passthrough-key",
                format!(
                    "{}: burrow cannot see the requests of a passthrough service (tls: passthrough); remove {key}, or let burrow handle TLS",
                    at.name(&[key])
                ),
            )
            .at(at.key(&[key]))
            .help("with tls: passthrough the app keeps its own HTTPS, so only hosts and tls apply"),
        );
    }
}

fn paths(site: &WebSite, at: &At<'_>, out: &mut Vec<Diagnostic>) {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (i, p) in site.paths.iter().enumerate() {
        let n = i.to_string();
        match route_path(p).and_then(|norm| Ok((route_key(p)?, norm))) {
            Ok((key, norm)) if seen.contains_key(&key) => {
                let first = &seen[&key];
                let message = if *first == norm {
                    format!("{} lists \"{norm}\" twice", at.name(&["paths"]))
                } else {
                    format!(
                        "{}: \"{first}\" and \"{norm}\" are the same path; paths match whatever the letter case",
                        at.name(&["paths"])
                    )
                };
                out.push(
                    Diagnostic::error("duplicate-path", message)
                        .at(at.value(&["paths", &n]))
                        .help("list each path once; /api already covers /api/..."),
                )
            }
            Ok((key, norm)) => {
                seen.insert(key, norm);
            }
            Err(e) => out.push(
                Diagnostic::error("invalid-path", format!("{}: {e}", at.name(&["paths"])))
                    .at(at.value(&["paths", &n]))
                    .help("a path covers itself and everything below it, so /api needs no /*"),
            ),
        }
    }
    let Some(app) = site.app_path.as_deref() else {
        return;
    };
    let name = at.name(&["app_path"]);
    let problem = if site.paths.is_empty() {
        Some(format!(
            "{name}: app_path needs paths: it replaces the matched path, like paths: [/api] with app_path: /"
        ))
    } else if !app.trim().starts_with('/') {
        Some(format!(
            "{name}: \"{}\" must start with /, like /v1",
            app.trim()
        ))
    } else {
        route_path(app).err().map(|e| format!("{name}: {e}"))
    };
    if let Some(m) = problem {
        out.push(
            Diagnostic::error("app-path", m)
                .at(at.value(&["app_path"]))
                .help(
                    "app_path: / removes the matched prefix; app_path: /v1 puts /v1 in its place",
                ),
        );
    }
}

pub(super) fn redirect_target(to: &str) -> bool {
    let t = to.trim();
    if t.starts_with('/') {
        return !t.starts_with("//") && !t.contains('\\') && http::HeaderValue::from_str(t).is_ok();
    }
    t.parse::<http::Uri>().is_ok_and(|u| {
        matches!(u.scheme_str(), Some("https" | "http")) && u.host().is_some_and(|h| !h.is_empty())
    })
}

pub(super) fn bad_target(to: &str) -> String {
    format!(
        "redirect target \"{}\" must be a full URL (https://...) or a path (/...)",
        to.trim()
    )
}

fn redirects(
    list: &BTreeMap<String, burrow_config::RedirectTarget>,
    at: &At<'_>,
    out: &mut Vec<Diagnostic>,
) {
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for (from, to) in list {
        let key = at.key(&["redirects", from]);
        match route_path(from) {
            Ok(norm) => {
                if let Some(first) = seen.insert(route_key(from).unwrap_or_default(), from) {
                    out.push(
                        Diagnostic::error(
                            "duplicate-path",
                            format!(
                                "{}: \"{first}\" and \"{from}\" are the same path {norm}",
                                at.name(&["redirects"])
                            ),
                        )
                        .at(key)
                        .help("keep one of them"),
                    );
                }
            }
            Err(e) => out.push(
                Diagnostic::error("invalid-path", format!("{}: {e}", at.name(&["redirects"])))
                    .at(key)
                    .help("a redirect covers its path and everything below it"),
            ),
        }
        if !redirect_target(to.to()) {
            out.push(
                Diagnostic::error("redirect-target", bad_target(to.to()))
                    .at(at.value(&["redirects", from]))
                    .help("for example /new, or https://example.net/new"),
            );
        }
    }
}

fn redirect_from(site: &WebSite, at: &At<'_>, out: &mut Vec<Diagnostic>) {
    if site.redirect_from.is_empty() {
        return;
    }
    let name = at.name(&["redirect_from"]);
    if let Some(first) = site.hosts.first()
        && is_wildcard(first.trim())
    {
        out.push(
            Diagnostic::error(
                "redirect-from",
                format!("{name}: the first host must be a plain name to redirect to"),
            )
            .at(at.key(&["redirect_from"]))
            .help("put a plain name like example.com first in hosts"),
        );
    }
    for (i, h) in site.redirect_from.iter().enumerate() {
        let n = i.to_string();
        let problem = match host_name(h, SingleLabel::Allowed) {
            Err(e) => Some(format!("{name}: {e}")),
            Ok(host)
                if site
                    .hosts
                    .iter()
                    .any(|s| host_name(s, SingleLabel::Allowed).is_ok_and(|s| s == host)) =>
            {
                Some(format!(
                    "{name}: \"{host}\" is also in hosts; a name either serves the site or redirects to it"
                ))
            }
            Ok(_) => None,
        };
        if let Some(m) = problem {
            out.push(
                Diagnostic::error("redirect-from", m)
                    .at(at.value(&["redirect_from", &n]))
                    .help("for example redirect_from: [www.example.com]"),
            );
        }
    }
}

fn target_tls(
    name: &str,
    site: &WebSite,
    target: &Target,
    at: &At<'_>,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) {
    match &site.target_tls {
        Some(TargetTls::Ca(c)) => {
            let ca = c.ca.as_deref().unwrap_or_default().trim();
            let problem = match std::fs::read(base_dir.join(ca)) {
                _ if ca.is_empty() => None,
                Err(e) => Some(format!("cannot read {ca}: {}", file_reason(&e))),
                Ok(pem) => not_a_ca(&pem).map(|e| format!("{ca} {e}")),
            };
            if let Some(p) = problem {
                out.push(
                    Diagnostic::error("target-ca", format!("{}: {p}", at.name(&["target_tls", "ca"])))
                        .at(at.value(&["target_tls", "ca"]))
                        .help("give the PEM file of the CA that signed the app's certificate; paths are relative to the config file"),
                );
            }
            let hp = target.host_port();
            if target.host().is_some_and(|h| is_ip(&h)) {
                out.push(
                    Diagnostic::warning(
                        "target-ip",
                        format!("target \"{hp}\" is an IP address; the app's certificate must name that IP"),
                    )
                    .at(at.value(&["target_tls"]))
                    .help("or give the target by the name its certificate holds"),
                );
            }
        }
        Some(TargetTls::Unchecked(u)) if u.insecure_skip_verify => out.push(
            Diagnostic::warning(
                "target-unchecked",
                format!(
                    "services.{name}: the app's certificate is not checked (insecure_skip_verify); use ca: for apps with a real certificate"
                ),
            )
            .at(at.value(&["target_tls"]))
            .help("fine for an app with a self-made certificate on a network you trust"),
        ),
        _ => {}
    }
}

struct Shared<'a> {
    max_body_size: Option<&'a str>,
    request_headers: &'a BTreeMap<String, String>,
    response_headers: &'a BTreeMap<String, String>,
    error_pages: Option<&'a str>,
}

fn shared(s: &Shared<'_>, at: &At<'_>, base_dir: &Path, out: &mut Vec<Diagnostic>) {
    if let Some(size) = s.max_body_size
        && let Err(e) = burrow_syntax::size::parse_size(size)
    {
        out.push(
            Diagnostic::error(
                "invalid-size",
                format!("{}: {e}", at.name(&["max_body_size"])),
            )
            .at(at.value(&["max_body_size"]))
            .help("for example 100MB or 8GB"),
        );
    }
    headers("request_headers", s.request_headers, at, out);
    headers("response_headers", s.response_headers, at, out);
    if let Some(dir) = s.error_pages {
        pages(dir.trim(), at, base_dir, out);
    }
}

fn headers(key: &str, map: &BTreeMap<String, String>, at: &At<'_>, out: &mut Vec<Diagnostic>) {
    let field = at.name(&[key]);
    for (name, value) in map {
        let problem = if http::HeaderName::from_bytes(name.trim().as_bytes()).is_err() {
            Some(format!(
                "{field}: \"{name}\" is not a header name; a name is letters, digits and any of ! # $ % & ' * + - . ^ _ ` | ~, with no spaces, like X-Frame-Options"
            ))
        } else if !burrow_syntax::web::headers::settable(name) {
            Some(format!(
                "{field}: \"{}\" belongs to HTTP's own framing, which burrow handles; it cannot be set",
                name.trim()
            ))
        } else if http::HeaderValue::from_str(value.trim()).is_err() {
            Some(format!(
                "{field}: the value of \"{}\" holds characters a header cannot carry",
                name.trim()
            ))
        } else {
            None
        };
        if let Some(m) = problem {
            out.push(
                Diagnostic::error("invalid-header", m)
                    .at(at.key(&[key, name]))
                    .help("write Name: value, and \"\" to remove a header"),
            );
        }
    }
}

fn pages(dir: &str, at: &At<'_>, base_dir: &Path, out: &mut Vec<Diagnostic>) {
    let field = at.name(&["error_pages"]);
    let span = at.value(&["error_pages"]);
    match std::fs::read_dir(base_dir.join(dir)) {
        Err(e) => out.push(
            Diagnostic::error(
                "error-pages",
                format!("{field}: cannot read the folder {dir}: {}", file_reason(&e)),
            )
            .at(span)
            .help("give a folder with any of 404.html, 413.html, 429.html, 502.html, 503.html, 504.html and maintenance.html, relative to the config file"),
        ),
        Ok(entries) => {
            let names: Vec<String> = entries
                .flatten()
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .collect();
            if !PAGES.iter().any(|p| names.iter().any(|n| n == p)) {
                out.push(
                    Diagnostic::warning(
                        "error-pages",
                        format!("{field}: {dir} holds none of {}; the built-in pages are used", PAGES.join(", ")),
                    )
                    .at(span)
                    .help("name each page after its status, like 404.html"),
                );
            }
        }
    }
}

#[cfg(feature = "web")]
fn not_a_ca(pem: &[u8]) -> Option<String> {
    burrow_web::tls::files::ca_roots(pem).err()
}

#[cfg(not(feature = "web"))]
fn not_a_ca(_pem: &[u8]) -> Option<String> {
    None
}

fn grpc_notes(name: &str, served: Served, http2_off: bool, at: &At<'_>, out: &mut Vec<Diagnostic>) {
    let field = format!("services.{name}.web.target_http2");
    let mut note = |message: String| {
        out.push(
            Diagnostic::warning("grpc-reach", message)
                .at(at.value(&["target_http2"]))
                .help("target_http2 also serves apps that only want HTTP/2 from burrow, so this is a note"),
        );
    };
    if http2_off {
        note(format!(
            "{field}: web.http2 is false, so gRPC clients cannot reach this site (gRPC needs HTTP/2); turn http2 on"
        ));
    }
    if served == Served::Plain {
        note(format!(
            "{field}: this site has tls: off; gRPC clients cannot connect, burrow accepts HTTP/2 from visitors only over TLS"
        ));
    }
}
