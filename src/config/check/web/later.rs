use super::Spans;
use crate::config::diagnostics::Diagnostic;
use burrow_config::{ServerConfig, ServiceWeb, WebConfig};

pub(super) const WEB_HTTPS_REDIRECT: &str = "web-https-redirect-later";
pub(super) const WEB_LETSENCRYPT: &str = "web-letsencrypt-later";
pub(super) const WEB_CERTIFICATES: &str = "web-certificates-later";
pub(super) const SITE_HTTPS_REDIRECT: &str = "site-https-redirect-later";

fn top_level(web: &WebConfig) -> [(&'static str, &'static str, bool); 3] {
    [
        (
            WEB_HTTPS_REDIRECT,
            "https_redirect",
            web.https_redirect.is_some(),
        ),
        (WEB_LETSENCRYPT, "letsencrypt", web.letsencrypt.is_some()),
        (
            WEB_CERTIFICATES,
            "certificates",
            !web.certificates.is_empty(),
        ),
    ]
}

fn not_yet(code: &'static str, key: &str, at: crate::config::tree::Span) -> Diagnostic {
    Diagnostic::error(code, format!("{key}: not in this build yet"))
        .at(at)
        .help("remove it for now")
}

pub(super) fn refuse(cfg: &ServerConfig, spans: &Spans, out: &mut Vec<Diagnostic>) {
    if let Some(web) = &cfg.web {
        for (code, key, written) in top_level(web) {
            if written || spans.has(&["web", key]) {
                out.push(not_yet(
                    code,
                    &format!("web.{key}"),
                    spans.key(&["web", key]),
                ));
            }
        }
    }
    for (name, svc) in &cfg.services {
        let path = ["services", name, "web", "https_redirect"];
        let written = svc.web.as_ref().and_then(ServiceWeb::https_redirect);
        if written.is_some() || spans.has(&path) {
            let key = format!("services.{name}.web.https_redirect");
            out.push(not_yet(SITE_HTTPS_REDIRECT, &key, spans.key(&path)));
        }
    }
}
