use std::path::Path;

use super::site_keys::At;
use super::{Served, Spans};
use crate::base::failure::file_reason;
use crate::config::diagnostics::Diagnostic;
use burrow_config::{ForwardAuthSetting, Protocol, ServerConfig, ServiceWeb, WebSite};
use burrow_syntax::passwd::{HashProblem, check_hash, valid_user};
use burrow_syntax::web::{SingleLabel, host_name};

pub(super) fn check_access(
    cfg: &ServerConfig,
    https: bool,
    tunnel: Option<&str>,
    base_dir: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
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
        if !site.basic_auth.is_empty() && site.forward_auth.as_ref().is_some_and(on) {
            out.push(
                Diagnostic::error(
                    "auth-both",
                    format!(
                        "services.{name}.web: basic_auth and forward_auth are both set; use one of them (the login service checks its own users)"
                    ),
                )
                .at(at.key(&["forward_auth"]))
                .help("keep forward_auth for single sign-on, or basic_auth for a few passwords"),
            );
        }
        passwords(name, site, served, &at, out);
        forward(cfg, name, site, served, &at, out);
        client_auth(name, site, served, tunnel, base_dir, &at, out);
    }
}

fn on(f: &ForwardAuthSetting) -> bool {
    !matches!(f, ForwardAuthSetting::Shared(false))
}

fn passwords(name: &str, site: &WebSite, served: Served, at: &At<'_>, out: &mut Vec<Diagnostic>) {
    let field = format!("services.{name}.web.basic_auth");
    for (user, hash) in &site.basic_auth {
        let problem = if !valid_user(user.trim()) {
            Some(format!(
                "{field}: user name \"{user}\" must not be empty or hold \":\""
            ))
        } else {
            match check_hash(hash) {
                Ok(()) => None,
                Err(HashProblem::Unknown) => Some(format!(
                    "{field}: the hash for \"{user}\" is not argon2id or bcrypt; make one with burrow passwd {user}"
                )),
                Err(HashProblem::OldBcrypt) => Some(format!(
                    "{field}: the hash for \"{user}\" is bcrypt $2x$, an old variant with a known bug; make a new one with burrow passwd {user}"
                )),
                Err(HashProblem::Argon2Variant(v)) => Some(format!(
                    "{field}: the hash for \"{user}\" is {v}; only argon2id is accepted"
                )),
                Err(HashProblem::TooCostly(what)) => Some(format!(
                    "{field}: the hash for \"{user}\" asks for more work than burrow allows ({what}); make one with burrow passwd {user}"
                )),
            }
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

fn forward(
    cfg: &ServerConfig,
    name: &str,
    site: &WebSite,
    served: Served,
    at: &At<'_>,
    out: &mut Vec<Diagnostic>,
) {
    let field = format!("services.{name}.web.forward_auth");
    let login = match &site.forward_auth {
        None | Some(ForwardAuthSetting::Shared(false)) => return,
        Some(ForwardAuthSetting::Shared(true)) => {
            match cfg.web.as_ref().and_then(|w| w.forward_auth.as_ref()) {
                Some(f) => f,
                None => {
                    out.push(
                    Diagnostic::error(
                        "forward-auth",
                        format!("{field}: true uses web.forward_auth, which is not set; add web.forward_auth: {{service, path}}, or give this site its own"),
                    )
                    .at(at.value(&["forward_auth"]))
                    .help("for example forward_auth: {service: authelia, path: /api/authz/forward-auth} under web"),
                );
                    return;
                }
            }
        }
        Some(ForwardAuthSetting::Own(f)) => f,
    };
    if login.service.trim() == name {
        out.push(
            Diagnostic::error(
                "forward-auth",
                format!("{field}: a service cannot check its own visitors; the login site must stay open"),
            )
            .at(at.value(&["forward_auth"]))
            .help("remove forward_auth from the login service"),
        );
    }
    if let Some(ServiceWeb::Site(login_site)) = cfg
        .services
        .get(login.service.trim())
        .and_then(|l| l.web.as_ref())
        && login_site.target_http2
        && login_site.target_tls.is_some()
    {
        out.push(
            Diagnostic::warning(
                "forward-auth-h2",
                format!(
                    "{field}: the login service \"{}\" speaks HTTP/2 only to its app (target_http2 with target_tls), but the login check asks over HTTP/1.1, so every check would fail with 502",
                    login.service.trim()
                ),
            )
            .at(at.value(&["forward_auth"]))
            .help("remove target_http2 from the login service, or let it accept HTTP/1.1"),
        );
    }
    if served == Served::Plain {
        out.push(
            Diagnostic::warning(
                "plain-password",
                format!(
                    "{field}: login cookies are sent unencrypted over plain HTTP; use HTTPS, or keep the site on a trusted network"
                ),
            )
            .at(at.key(&["forward_auth"]))
            .help("anyone on the path can read a login cookie sent over http://"),
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
    for (key, file) in
        std::iter::once(("ca", Some(&c.ca))).chain(std::iter::once(("crl", c.crl.as_ref())))
    {
        let Some(file) = file.map(|f| f.trim()) else {
            continue;
        };
        if let Err(e) = std::fs::read(base_dir.join(file)) {
            out.push(
                Diagnostic::error(
                    "client-auth",
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
        &base_dir.join(c.ca.trim()),
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
