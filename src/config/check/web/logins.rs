use super::site_keys::At;
use super::{Served, Spans};
use crate::base::suggest::closest;
use crate::config::check::{name_error, valid_name};
use crate::config::diagnostics::Diagnostic;
use burrow_admin_proto::codes::{
    FORWARD_AUTH, FORWARD_AUTH_H2, FORWARD_AUTH_UNUSED, UNKNOWN_LOGIN,
};
use burrow_config::{
    ForwardAuthConfig, ForwardAuthSetting, LoginPick, Protocol, RESERVED_LOGIN_NAMES, ServerConfig,
    ServiceWeb, WebSite,
};
use std::collections::BTreeMap;

const LOGINS: &str = "web.forward_auth";

fn logins(cfg: &ServerConfig) -> &BTreeMap<String, ForwardAuthConfig> {
    static NONE: BTreeMap<String, ForwardAuthConfig> = BTreeMap::new();
    cfg.web.as_ref().map_or(&NONE, |w| &w.forward_auth)
}

fn sites(cfg: &ServerConfig) -> impl Iterator<Item = (&String, &WebSite)> {
    cfg.services
        .iter()
        .filter_map(|(name, svc)| match svc.web.as_ref() {
            Some(ServiceWeb::Site(site)) if svc.protocol == Protocol::Tcp => Some((name, &**site)),
            _ => None,
        })
}

pub(super) fn check_names(cfg: &ServerConfig, spans: &Spans, out: &mut Vec<Diagnostic>) {
    let all = logins(cfg);
    let at = At {
        spans,
        base: vec!["web".into(), "forward_auth".into()],
    };
    for login in all.keys() {
        if RESERVED_LOGIN_NAMES.contains(&login.as_str()) {
            out.push(
                Diagnostic::error(
                    FORWARD_AUTH,
                    format!(
                        "{LOGINS}.{login}: true and false cannot name a login service; they mean on and off"
                    ),
                )
                .at(at.key(&[login.as_str()]))
                .help("rename it, for example to authelia"),
            );
            continue;
        }
        if !valid_name(login) {
            out.push(name_error(
                "login service",
                login,
                at.key(&[login.as_str()]),
            ));
            continue;
        }
        let used = sites(cfg).any(|(_, s)| {
            s.forward_auth.as_ref().is_some_and(
                |f| matches!(f.pick(all), LoginPick::Found(n, _) if n == login.as_str()),
            )
        });
        if !used {
            out.push(
                Diagnostic::warning(
                    FORWARD_AUTH_UNUSED,
                    format!(
                        "{LOGINS}.{login}: no website uses it; set forward_auth: {login} on a site, or remove it"
                    ),
                )
                .at(at.key(&[login.as_str()])),
            );
        }
    }
}

fn picked<'a>(
    cfg: &'a ServerConfig,
    field: &str,
    setting: &'a ForwardAuthSetting,
    at: &At<'_>,
    out: &mut Vec<Diagnostic>,
) -> Option<(&'a str, &'a ForwardAuthConfig)> {
    let all = logins(cfg);
    let names: Vec<&str> = all.keys().map(String::as_str).collect();
    let (code, message, help) = match setting.pick(all) {
        LoginPick::Off => return None,
        LoginPick::Found(n, f) => return Some((n, f)),
        LoginPick::Unknown(n) if names.is_empty() => (
            UNKNOWN_LOGIN,
            format!("{field}: there is no login service \"{n}\" under {LOGINS}; none are defined"),
            format!("add {LOGINS}: {{{n}: {{service, path}}}}"),
        ),
        LoginPick::Unknown(n) => (
            UNKNOWN_LOGIN,
            format!(
                "{field}: there is no login service \"{n}\" under {LOGINS}; the names are {}",
                names.join(", ")
            ),
            match closest(n, names.iter().copied()) {
                Some(s) => format!("did you mean \"{s}\"?"),
                None => format!("pick one of them, or add it under {LOGINS}"),
            },
        ),
        LoginPick::NoneDefined => (
            FORWARD_AUTH,
            format!(
                "{field}: true uses {LOGINS}, which is not set; add {LOGINS}: {{authelia: {{service, path}}}}"
            ),
            format!(
                "for example {LOGINS}: {{authelia: {{service: authelia, path: /api/authz/forward-auth}}}}"
            ),
        ),
        LoginPick::Unclear => (
            FORWARD_AUTH,
            format!(
                "{field}: true is unclear with {} login services under {LOGINS}; name one: {}",
                names.len(),
                or_list(&names)
            ),
            format!("for example forward_auth: {}", names[0]),
        ),
    };
    out.push(
        Diagnostic::error(code, message)
            .at(at.value(&["forward_auth"]))
            .help(help),
    );
    None
}

fn or_list(names: &[&str]) -> String {
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => names.join(""),
    }
}

pub(super) fn forward(
    cfg: &ServerConfig,
    name: &str,
    site: &WebSite,
    served: Served,
    at: &At<'_>,
    out: &mut Vec<Diagnostic>,
) {
    let field = format!("services.{name}.web.forward_auth");
    let Some(setting) = site.forward_auth.as_ref().filter(|f| f.on()) else {
        return;
    };
    if let Some(answers) = logins(cfg)
        .iter()
        .find(|(_, l)| l.service.trim() == name)
        .map(|(n, _)| n)
    {
        out.push(
            Diagnostic::error(
                FORWARD_AUTH,
                format!(
                    "{field}: {name} answers the logins of {LOGINS}.{answers}, so its own site must stay open"
                ),
            )
            .at(at.value(&["forward_auth"]))
            .help("remove forward_auth from the login service"),
        );
    }
    let Some((login, f)) = picked(cfg, &field, setting, at, out) else {
        return;
    };
    let service = f.service.trim();
    if let Some(ServiceWeb::Site(login_site)) =
        cfg.services.get(service).and_then(|l| l.web.as_ref())
        && login_site.target_http2
        && login_site.target_tls.is_some()
    {
        out.push(
            Diagnostic::warning(
                FORWARD_AUTH_H2,
                format!(
                    "{field}: the login service \"{login}\" ({service}) speaks HTTP/2 only to its app (target_http2 with target_tls), but the login check asks over HTTP/1.1, so every check would fail with 502"
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
