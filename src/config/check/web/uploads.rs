use std::path::Path;

use super::Spans;
use crate::config::diagnostics::Diagnostic;
use burrow_config::{ServerConfig, ServiceWeb, SiteTls, TargetTls};
use burrow_syntax::upload::Kind;

struct Slot<'a> {
    place: String,
    at: Vec<String>,
    state: Option<&'a Path>,
}

impl Slot<'_> {
    fn span(&self, spans: &Spans) -> crate::config::tree::Span {
        let p: Vec<&str> = self.at.iter().map(String::as_str).collect();
        spans.value(&p)
    }

    fn error(&self, spans: &Spans, message: String, help: &str) -> Diagnostic {
        Diagnostic::error("certificate-source", format!("{}: {message}", self.place))
            .at(self.span(spans))
            .help(help.to_string())
    }

    fn file(
        &self,
        id: &str,
        kind: Kind,
        spans: &Spans,
        out: &mut Vec<Diagnostic>,
    ) -> Option<String> {
        if burrow_syntax::upload::kind_of(id.trim()) != Some(kind) {
            out.push(self.error(
                spans,
                format!("\"{}\" is not an upload of {}", id.trim(), kind.what()),
                "upload the file again in the admin UI",
            ));
            return None;
        }
        let Some(state) = self.state else {
            out.push(self.error(
                spans,
                "uploads live in the server's state folder, which is not known here".into(),
                "check the config file itself, like burrow validate server.yaml",
            ));
            return None;
        };
        let path = burrow_web::tls::uploads::path(state, id.trim())?;
        if !path.is_file() {
            out.push(self.error(
                spans,
                format!(
                    "the upload {} is not in {}",
                    id.trim(),
                    burrow_web::tls::uploads::folder(state).display()
                ),
                "upload the file again in the admin UI",
            ));
            return None;
        }
        Some(path.display().to_string())
    }
}

fn pair(
    slot: &Slot<'_>,
    (cert, key): (&mut Option<String>, &mut Option<String>),
    upload: Option<&str>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let both = "set cert and key, or upload a certificate, not both";
    match (cert.is_some(), key.is_some(), upload) {
        (false, false, None) => out.push(slot.error(
            spans,
            "needs cert and key, or an uploaded certificate".into(),
            "for example { cert: certs/fullchain.pem, key: certs/privkey.pem }",
        )),
        (true, false, None) | (false, true, None) => out.push(slot.error(
            spans,
            "cert and key go together".into(),
            "give both files, the certificate chain and its private key",
        )),
        (true, true, None) => {}
        (_, _, Some(_)) if cert.is_some() || key.is_some() => {
            out.push(slot.error(spans, both.into(), "keep one of them"));
            *cert = None;
            *key = None;
        }
        (_, _, Some(id)) => {
            let file = slot.file(id, Kind::Pair, spans, out);
            *key = file.clone();
            *cert = file;
        }
    }
}

fn one(
    slot: &Slot<'_>,
    path: &mut Option<String>,
    (upload, kind): (Option<&str>, Kind),
    required: bool,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    match (path.is_some(), upload) {
        (true, Some(_)) => {
            out.push(slot.error(
                spans,
                "set the file or upload it, not both".into(),
                "keep one of them",
            ));
            *path = None;
        }
        (false, Some(id)) => *path = slot.file(id, kind, spans, out),
        (false, None) if required => out.push(slot.error(
            spans,
            format!("needs {}: a path, or an upload", kind.what()),
            "for example ca: certs/ca.pem",
        )),
        _ => {}
    }
}

fn site(
    name: &str,
    s: &mut burrow_config::WebSite,
    state: Option<&Path>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let slot = |key: &str| Slot {
        place: format!("services.{name}.web.{key}"),
        at: ["services", name, "web", key].map(String::from).to_vec(),
        state,
    };
    if let Some(SiteTls::Files(f)) = &mut s.tls {
        let upload = f.upload.clone();
        pair(
            &slot("tls"),
            (&mut f.cert, &mut f.key),
            upload.as_deref(),
            spans,
            out,
        );
    }
    if let Some(TargetTls::Ca(c)) = &mut s.target_tls {
        let upload = c.ca_upload.clone();
        one(
            &slot("target_tls"),
            &mut c.ca,
            (upload.as_deref(), Kind::Ca),
            true,
            spans,
            out,
        );
    }
    if let Some(c) = &mut s.client_auth {
        let (ca, crl) = (c.ca_upload.clone(), c.crl_upload.clone());
        one(
            &slot("client_auth"),
            &mut c.ca,
            (ca.as_deref(), Kind::Ca),
            true,
            spans,
            out,
        );
        one(
            &slot("client_auth"),
            &mut c.crl,
            (crl.as_deref(), Kind::Crl),
            false,
            spans,
            out,
        );
    }
}

pub(in crate::config::check) fn resolve(
    cfg: &ServerConfig,
    state: Option<&Path>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> ServerConfig {
    let mut cfg = cfg.clone();
    for (name, svc) in cfg.services.iter_mut() {
        if let Some(ServiceWeb::Site(s)) = &mut svc.web {
            site(name, s, state, spans, out);
        }
    }
    let Some(web) = cfg.web.as_mut() else {
        return cfg;
    };
    for (i, c) in web.certificates.iter_mut().enumerate() {
        let n = i.to_string();
        let slot = Slot {
            place: format!("web.certificates[{i}]"),
            at: ["web", "certificates", &n].map(String::from).to_vec(),
            state,
        };
        let upload = c.upload.clone();
        pair(
            &slot,
            (&mut c.cert, &mut c.key),
            upload.as_deref(),
            spans,
            out,
        );
    }
    if let Some(le) = web.letsencrypt.as_mut() {
        let slot = Slot {
            place: "web.letsencrypt.ca".into(),
            at: ["web", "letsencrypt"].map(String::from).to_vec(),
            state,
        };
        let upload = le.ca_upload.clone();
        one(
            &slot,
            &mut le.ca,
            (upload.as_deref(), Kind::Ca),
            false,
            spans,
            out,
        );
    }
    cfg
}
