use std::path::Path;

use super::site_keys::At;
use super::{Served, Spans};
use crate::base::failure::file_reason;
use crate::config::diagnostics::Diagnostic;
use burrow_config::{Bind, ForwardAuthSetting, Protocol, ServerConfig, ServiceWeb, WebSite};
use burrow_syntax::passwd::valid_user;
use burrow_syntax::web::{SingleLabel, host_name};

pub(super) fn check_access(
    cfg: &ServerConfig,
    https: bool,
    tunnel: Option<&str>,
    base_dir: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    super::logins::check_names(cfg, spans, out);
    super::oidc::check_providers(cfg, base_dir, spans, out);
    for (name, svc) in &cfg.services {
        let Some(web @ ServiceWeb::Site(site)) = svc.web.as_ref() else {
            continue;
        };
        if svc.protocol != Protocol::Tcp {
            continue;
        }
        let served = Served::of(web, https);
        if served == Served::Passthrough {
            continue;
        }
        let at = At {
            spans,
            base: vec!["services".into(), name.clone(), "web".into()],
        };
        if !site.basic_auth.is_empty()
            && site
                .forward_auth
                .as_ref()
                .is_some_and(ForwardAuthSetting::on)
        {
            out.push(
                Diagnostic::error(
                    burrow_admin_proto::codes::AUTH_BOTH,
                    format!(
                        "services.{name}.web: basic_auth and forward_auth are both set; use one of them (the login service checks its own users)"
                    ),
                )
                .at(at.key(&["forward_auth"]))
                .help("keep forward_auth for single sign-on, or basic_auth for a few passwords"),
            );
        }
        if let Some(bind) = svc.bind.as_ref().filter(|_| svc.allow_from.is_empty()) {
            let svc_at = At {
                spans,
                base: vec!["services".into(), name.clone()],
            };
            public_port(name, bind, site, &svc_at, out);
        }
        passwords(name, site, served, &at, out);
        super::logins::forward(cfg, name, site, served, &at, out);
        let fallback = cfg.web.as_ref().and_then(|w| w.https_redirect);
        let s = super::oidc::SiteAt {
            name,
            site,
            served,
            https_redirect: site.https_redirect.or(fallback),
        };
        super::oidc::check_site(cfg, &s, &at, out);
        client_auth(name, site, served, tunnel, base_dir, &at, out);
        if served != Served::Plain {
            devices_on_port_80(cfg, name, site, &at, out);
        }
    }
}

fn devices_on_port_80(
    cfg: &ServerConfig,
    name: &str,
    site: &WebSite,
    at: &At<'_>,
    out: &mut Vec<Diagnostic>,
) {
    let fallback = cfg.web.as_ref().and_then(|w| w.https_redirect);
    if site.client_auth.is_none() || site.https_redirect.or(fallback) != Some(false) {
        return;
    }
    let (from, fix) = if site.https_redirect.is_some() {
        ("https_redirect: false", "remove https_redirect: false")
    } else {
        (
            "web.https_redirect: false",
            "set https_redirect: true on this site, which keeps the other sites on plain http://, remove web.https_redirect: false",
        )
    };
    out.push(
        Diagnostic::error(
            "client-auth",
            format!(
                "services.{name}.web.client_auth: {from} would serve this site over plain http:// too, where no device certificate can be asked; {fix}, or remove client_auth"
            ),
        )
        .at(at.key(&["client_auth"]))
        .help("device certificates are checked in the HTTPS handshake, so port 80 always sends this site to https://"),
    );
}

fn loopback(bind: &Bind) -> bool {
    let Bind::Addr(a) = bind else {
        return false;
    };
    let host = a.rsplit_once(':').map_or(a.as_str(), |(h, _)| h);
    host.trim_matches(|c| c == '[' || c == ']')
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

fn public_port(name: &str, bind: &Bind, site: &WebSite, at: &At<'_>, out: &mut Vec<Diagnostic>) {
    let logins: Vec<&str> = [
        (!site.basic_auth.is_empty()).then_some("basic_auth"),
        site.forward_auth
            .as_ref()
            .filter(|f| f.on())
            .map(|_| "forward_auth"),
        site.client_auth.as_ref().map(|_| "client_auth"),
        site.oidc.as_ref().filter(|o| o.on()).map(|_| "oidc"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if logins.is_empty() || loopback(bind) {
        return;
    }
    out.push(
        Diagnostic::warning(
            burrow_admin_proto::codes::PUBLIC_PORT_SKIPS_LOGIN,
            format!(
                "services.{name}.bind: bind {bind} reaches the app directly, without the website's {}; remove bind, or limit it with allow_from",
                logins.join(" and ")
            ),
        )
        .at(at.value(&["bind"]))
        .help("visitors still need their burrow login; allow_from limits both the website and the port"),
    );
}

#[cfg(feature = "web")]
fn hash_problem(user: &str, hash: &str) -> Option<String> {
    use burrow_web::auth::{HashProblem, check_hash};
    let e = check_hash(hash).err()?;
    Some(match e {
        HashProblem::Argon2Variant(_) => e.to_string(),
        HashProblem::Bcrypt2x => format!("{e}; make a new one with burrow passwd {user}"),
        _ => format!("{e}; make one with burrow passwd {user}"),
    })
}

#[cfg(not(feature = "web"))]
fn hash_problem(_user: &str, _hash: &str) -> Option<String> {
    None
}

fn passwords(name: &str, site: &WebSite, served: Served, at: &At<'_>, out: &mut Vec<Diagnostic>) {
    let field = format!("services.{name}.web.basic_auth");
    for (user, hash) in &site.basic_auth {
        let problem = if !valid_user(user.trim()) {
            Some(format!(
                "{field}: user name \"{user}\" must not be empty or hold \":\""
            ))
        } else {
            hash_problem(user, hash).map(|why| format!("{field}: the hash for \"{user}\" {why}"))
        };
        if let Some(m) = problem {
            out.push(
                Diagnostic::error("basic-auth", m)
                    .at(at.value(&["basic_auth", user]))
                    .help("burrow passwd USER prints an argon2id hash to paste here"),
            );
        }
    }
    if served == Served::Plain && !site.basic_auth.is_empty() {
        out.push(
            Diagnostic::warning(
                "plain-password",
                format!(
                    "{field}: passwords are sent unencrypted over plain HTTP; use HTTPS, or keep the site on a trusted network"
                ),
            )
            .at(at.key(&["basic_auth"]))
            .help("anyone on the path can read a password sent over http://"),
        );
    }
}

fn client_auth(
    name: &str,
    site: &WebSite,
    served: Served,
    tunnel: Option<&str>,
    base_dir: &Path,
    at: &At<'_>,
    out: &mut Vec<Diagnostic>,
) {
    let Some(c) = &site.client_auth else {
        return;
    };
    let field = format!("services.{name}.web.client_auth");
    if served == Served::Plain {
        let off = matches!(
            site.tls,
            Some(burrow_config::SiteTls::Mode(burrow_config::TlsMode::Off))
        );
        let (code, m) = if off {
            (
                "plain-tls-key",
                format!("{field}: a site with tls: off has no HTTPS; remove client_auth"),
            )
        } else {
            (
                super::choices::NEEDS_HTTPS,
                super::choices::needs_https(name, "client_auth", "client_auth"),
            )
        };
        out.push(
            Diagnostic::error(code, m)
                .at(at.key(&["client_auth"]))
                .help("device certificates are checked in the HTTPS handshake"),
        );
        return;
    }
    if let Some(t) = tunnel
        && site
            .hosts
            .iter()
            .filter_map(|h| host_name(h, SingleLabel::Allowed).ok())
            .any(|h| h == t)
    {
        out.push(
            Diagnostic::error(
                "client-auth-tunnel",
                format!(
                    "{field}: \"{t}\" also carries the tunnel (websocket.host, by path); burrow clients have no device certificate, so they could not connect. Give the tunnel its own name, or remove client_auth"
                ),
            )
            .at(at.key(&["client_auth"]))
            .help("for example websocket.host: tunnel.example.com"),
        );
    }
    if c.ca.is_none() {
        return;
    }
    for (key, file) in
        std::iter::once(("ca", c.ca.as_ref())).chain(std::iter::once(("crl", c.crl.as_ref())))
    {
        let Some(file) = file.map(|f| f.trim()) else {
            continue;
        };
        if let Err(e) = std::fs::read(base_dir.join(file)) {
            out.push(
                Diagnostic::error(
                    burrow_admin_proto::codes::TLS_UNREADABLE,
                    format!("{field}.{key}: cannot read {file}: {}", file_reason(&e)),
                )
                .at(at.value(&["client_auth", key]))
                .help("paths are relative to the config file"),
            );
            return;
        }
    }
    files(&field, c, base_dir, at, out);
}

#[cfg(feature = "web")]
fn files(
    field: &str,
    c: &burrow_config::ClientAuthFiles,
    base_dir: &Path,
    at: &At<'_>,
    out: &mut Vec<Diagnostic>,
) {
    use burrow_web::tls::client_auth::{ClientAuthProblem, check_files};
    let crl = c.crl.as_ref().map(|p| base_dir.join(p.trim()));
    match check_files(
        &base_dir.join(c.ca.as_deref().unwrap_or_default().trim()),
        crl.as_deref(),
        std::time::SystemTime::now(),
    ) {
        Ok(notes) => {
            for n in notes {
                out.push(
                    Diagnostic::warning("client-auth-crl", format!("{field}.crl: {n}"))
                        .at(at.value(&["client_auth", "crl"]))
                        .help("publish a new CRL from your CA, like openssl ca -gencrl"),
                );
            }
        }
        Err(p) => {
            let key = match p {
                ClientAuthProblem::CaUnreadable(_) | ClientAuthProblem::NotCa => "ca",
                _ => "crl",
            };
            out.push(
                Diagnostic::error("client-auth", format!("{field}.{key}: {p}"))
                    .at(at.value(&["client_auth", key]))
                    .help("ca holds your device CA (and its intermediates); crl holds the revocation lists it publishes"),
            );
        }
    }
}

#[cfg(not(feature = "web"))]
fn files(
    _field: &str,
    _c: &burrow_config::ClientAuthFiles,
    _base_dir: &Path,
    _at: &At<'_>,
    _out: &mut Vec<Diagnostic>,
) {
}
