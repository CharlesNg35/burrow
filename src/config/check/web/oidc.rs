use std::path::Path;

use super::dns_secrets::{Field, check as check_secret};
use super::site_keys::At;
use super::{Served, Spans};
use crate::base::suggest::closest;
use crate::config::check::{name_error, valid_name};
use crate::config::diagnostics::Diagnostic;
use burrow_admin_proto::codes::{
    ALLOW_USERS, AUTH_BOTH, OIDC, OIDC_SECRET, OIDC_UNUSED, UNKNOWN_OIDC,
};
use burrow_config::{
    ForwardAuthSetting, OidcConfig, OidcPick, Protocol, RESERVED_LOGIN_NAMES, ServerConfig,
    ServiceWeb, WebSite,
};
use burrow_syntax::web::oidc::{allow_entry, issuer_problem};
use std::collections::BTreeMap;

const PROVIDERS: &str = "web.oidc";

fn providers(cfg: &ServerConfig) -> &BTreeMap<String, OidcConfig> {
    static NONE: BTreeMap<String, OidcConfig> = BTreeMap::new();
    cfg.web.as_ref().map_or(&NONE, |w| &w.oidc)
}

fn uses(cfg: &ServerConfig, name: &str) -> bool {
    let all = providers(cfg);
    cfg.services.values().any(|svc| match svc.web.as_ref() {
        Some(ServiceWeb::Site(s)) if svc.protocol == Protocol::Tcp => s
            .oidc
            .as_ref()
            .is_some_and(|o| matches!(o.pick(all), OidcPick::Found(n) if n.contains(&name))),
        _ => false,
    })
}

pub(super) fn check_providers(
    cfg: &ServerConfig,
    base_dir: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let at = At {
        spans,
        base: vec!["web".into(), "oidc".into()],
    };
    for (name, p) in providers(cfg) {
        let key = at.key(&[name.as_str()]);
        if RESERVED_LOGIN_NAMES.contains(&name.as_str()) {
            out.push(
                Diagnostic::error(
                    OIDC,
                    format!("{PROVIDERS}.{name}: true and false cannot name a login provider; they mean on and off"),
                )
                .at(key)
                .help("rename it, for example to google"),
            );
            continue;
        }
        if !valid_name(name) {
            out.push(name_error("login provider", name, key));
            continue;
        }
        provider(name, p, &at, base_dir, out);
        if !uses(cfg, name) {
            out.push(
                Diagnostic::warning(
                    OIDC_UNUSED,
                    format!("{PROVIDERS}.{name}: no website uses it; set oidc: {name} on a site, or remove it"),
                )
                .at(at.key(&[name.as_str()])),
            );
        }
    }
}

fn provider(name: &str, p: &OidcConfig, at: &At<'_>, base_dir: &Path, out: &mut Vec<Diagnostic>) {
    let field = format!("{PROVIDERS}.{name}");
    if let Some(why) = issuer_problem(&p.issuer) {
        out.push(
            Diagnostic::error(OIDC, format!("{field}.issuer {why}"))
                .at(at.value(&[name, "issuer"]))
                .help("the provider's issuer URL, for example https://accounts.google.com"),
        );
    } else if p.issuer.trim().starts_with("http://") {
        out.push(
            Diagnostic::warning(
                OIDC,
                format!("{field}.issuer is plain http://; burrow reaches it only on loopback or private addresses, and the client secret travels unencrypted"),
            )
            .at(at.value(&[name, "issuer"]))
            .help("use https:// unless the provider sits on your own network"),
        );
    }
    if p.client_id.trim().is_empty() {
        out.push(
            Diagnostic::error(OIDC, format!("{field}.client_id is empty"))
                .at(at.value(&[name, "client_id"]))
                .help("the client ID the provider gave you when you registered burrow"),
        );
    }
    if let Some(secret) = &p.client_secret {
        let f = Field {
            path: vec![
                "web".into(),
                "oidc".into(),
                name.into(),
                "client_secret".into(),
            ],
            name: format!("{field}.client_secret"),
            value: secret,
            env: "OIDC_SECRET",
            inline_ok: false,
            code: OIDC_SECRET,
        };
        out.extend(check_secret(&f, base_dir, at.spans));
    }
}

fn or_list(names: &[&str]) -> String {
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => names.join(""),
    }
}

fn picked(
    cfg: &ServerConfig,
    field: &str,
    site: &WebSite,
) -> Option<(&'static str, String, String)> {
    let all = providers(cfg);
    let names: Vec<&str> = all.keys().map(String::as_str).collect();
    let pick = site.oidc.as_ref()?.pick(all);
    Some(match pick {
        OidcPick::Off | OidcPick::Found(_) => return None,
        OidcPick::Unknown(n) if names.is_empty() => (
            UNKNOWN_OIDC,
            format!(
                "{field}: there is no login provider \"{n}\" under {PROVIDERS}; none are defined"
            ),
            format!("add {PROVIDERS}: {{{n}: {{issuer, client_id, client_secret}}}}"),
        ),
        OidcPick::Unknown(n) => (
            UNKNOWN_OIDC,
            format!(
                "{field}: there is no login provider \"{n}\" under {PROVIDERS}; the names are {}",
                names.join(", ")
            ),
            match closest(n, names.iter().copied()) {
                Some(s) => format!("did you mean \"{s}\"?"),
                None => format!("pick one of them, or add it under {PROVIDERS}"),
            },
        ),
        OidcPick::NoneDefined => (
            OIDC,
            format!("{field}: true uses {PROVIDERS}, which is not set"),
            format!(
                "for example {PROVIDERS}: {{google: {{issuer: https://accounts.google.com, client_id: ..., client_secret: ${{GOOGLE_SECRET}}}}}}"
            ),
        ),
        OidcPick::Unclear => (
            OIDC,
            format!(
                "{field}: true is unclear with {} login providers under {PROVIDERS}; name one or a list: {}",
                names.len(),
                or_list(&names)
            ),
            format!("for example oidc: {}", names[0]),
        ),
        OidcPick::Empty => (
            OIDC,
            format!("{field}: the list is empty"),
            "name a provider, or remove oidc".to_string(),
        ),
    })
}

pub(super) struct SiteAt<'a> {
    pub name: &'a str,
    pub site: &'a WebSite,
    pub served: Served,
    pub https_redirect: Option<bool>,
}

pub(super) fn check_site(
    cfg: &ServerConfig,
    s: &SiteAt<'_>,
    at: &At<'_>,
    out: &mut Vec<Diagnostic>,
) {
    let (name, site) = (s.name, s.site);
    let field = format!("services.{name}.web.oidc");
    let Some(_) = site.oidc.as_ref().filter(|o| o.on()) else {
        if !site.allow_users.is_empty() {
            out.push(
                Diagnostic::warning(ALLOW_USERS, format!("services.{name}.web.allow_users: nobody logs in without oidc, so it does nothing"))
                    .at(at.key(&["allow_users"]))
                    .help("set oidc: <provider> on this site, or remove allow_users"),
            );
        }
        return;
    };
    let err = |code: &'static str, message: String, help: &str| {
        Diagnostic::error(code, message)
            .at(at.key(&["oidc"]))
            .help(help.to_string())
    };
    if let Some((code, message, help)) = picked(cfg, &field, site) {
        out.push(
            Diagnostic::error(code, message)
                .at(at.value(&["oidc"]))
                .help(help),
        );
    }
    let other = match (&site.basic_auth, &site.forward_auth) {
        (b, _) if !b.is_empty() => Some("basic_auth"),
        (_, Some(f)) if ForwardAuthSetting::on(f) => Some("forward_auth"),
        _ => None,
    };
    if let Some(other) = other {
        out.push(err(
            AUTH_BOTH,
            format!("services.{name}.web: {other} and oidc are both set; use one kind of login"),
            "keep oidc for logins with accounts like Google, or the other one",
        ));
    }
    if s.served == Served::Plain {
        out.push(err(OIDC, format!("{field}: this site is served as plain HTTP (tls: off, or no web.https), and login cookies need HTTPS; serve it over HTTPS, or remove oidc"), "browsers keep a login cookie only for https:// sites"));
    } else if s.https_redirect == Some(false) {
        out.push(err(OIDC, format!("{field}: https_redirect: false would serve this site over plain http:// too, where the login cookie is never sent; remove it, or remove oidc"), "a site with oidc always sends http:// visitors to https://"));
    }
    allow_users(name, site, at, out);
}

fn allow_users(name: &str, site: &WebSite, at: &At<'_>, out: &mut Vec<Diagnostic>) {
    let field = format!("services.{name}.web.allow_users");
    if site.allow_users.is_empty() {
        out.push(
            Diagnostic::error(
                ALLOW_USERS,
                format!("{field}: nobody may log in; list who may, as emails or @domain"),
            )
            .at(at.key(&["oidc"]))
            .help("for example allow_users: [alice@gmail.com, \"@mycompany.com\"]"),
        );
    }
    for (i, u) in site.allow_users.iter().enumerate() {
        if let Err(why) = allow_entry(u) {
            out.push(
                Diagnostic::error(ALLOW_USERS, format!("{field}: \"{}\" {why}", u.trim()))
                    .at(at.value(&["allow_users", &i.to_string()]))
                    .help("write alice@gmail.com for one person, or \"@mycompany.com\" (quoted) for everyone there"),
            );
        }
    }
}
