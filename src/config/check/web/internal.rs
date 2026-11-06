use super::Spans;
use crate::config::diagnostics::Diagnostic;
use burrow_admin_proto::codes::INTERNAL_NAME;
use burrow_config::{ServerConfig, ServiceWeb, SiteTls, TlsMode};
use burrow_syntax::address::is_localhost;
use burrow_syntax::web::local_ca::{NotLocal, PERMITTED_NAMES, local_ca_refuses};
use burrow_syntax::web::{SingleLabel, host_name};

pub(super) fn problem(host: &str) -> Option<String> {
    let suffixes = PERMITTED_NAMES.join(", ");
    Some(match local_ca_refuses(host)? {
        NotLocal::PublicAddress => format!(
            "{host} is a public address; tls: internal serves only addresses of your own network, like 192.168.1.5 or fd00::5"
        ),
        NotLocal::WideWildcard => format!(
            "{host} spans a whole suffix or a public domain; a local CA wildcard is one label under a LAN name, like *.nas.lan"
        ),
        NotLocal::PublicName => {
            let short = host.split('.').next().unwrap_or(host);
            format!(
                "{host} is not a LAN name; tls: internal is only for names under {suffixes}, so use tls: auto for public names, or name it like {short}.lan"
            )
        }
    })
}

pub(super) fn check_internal(cfg: &ServerConfig, spans: &Spans, out: &mut Vec<Diagnostic>) {
    for (name, svc) in &cfg.services {
        let Some(ServiceWeb::Site(site)) = &svc.web else {
            continue;
        };
        if site.tls != Some(SiteTls::Mode(TlsMode::Internal)) {
            continue;
        }
        let names = site
            .hosts
            .iter()
            .map(|h| ("hosts", h))
            .chain(site.redirect_from.iter().map(|h| ("redirect_from", h)));
        let mut index = (0usize, 0usize);
        for (key, raw) in names {
            let i = if key == "hosts" {
                &mut index.0
            } else {
                &mut index.1
            };
            let at = i.to_string();
            *i += 1;
            let single = if is_localhost(raw) {
                SingleLabel::Allowed
            } else {
                SingleLabel::Refused
            };
            let Ok(host) = host_name(raw, single) else {
                continue;
            };
            if let Some(m) = problem(&host) {
                out.push(
                    Diagnostic::error(INTERNAL_NAME, format!("services.{name}.web.{key}: {m}"))
                        .about(&host)
                        .at(spans.value(&["services", name, "web", key, &at]))
                        .help("the local CA's root only allows LAN names, so a leaked key cannot sign for the internet"),
                );
            }
        }
    }
}
