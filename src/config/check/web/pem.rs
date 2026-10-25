use std::path::Path;

use super::Spans;
use crate::base::failure::file_reason;
use crate::config::diagnostics::Diagnostic;
use crate::config::tree::Span;
use crate::wire::transport::tls::{parse_certs, parse_key};
use burrow_config::{ServerConfig, ServiceWeb, SiteTls};
use burrow_syntax::web::{SingleLabel, host_name};
use burrow_web::tls::files::{certified, covers, dns_names};
use tokio_rustls::rustls::sign::CertifiedKey;

struct Pair<'a> {
    place: String,
    at: Span,
    cert: &'a str,
    key: &'a str,
}

fn load(pair: &Pair<'_>, base: &Path, out: &mut Vec<Diagnostic>) -> Option<CertifiedKey> {
    let fail = |message: String, help: &str| {
        Diagnostic::error("certificate-file", format!("{}: {message}", pair.place))
            .at(pair.at)
            .help(help.to_string())
    };
    let read =
        |field: &str, file: &str, out: &mut Vec<Diagnostic>| match std::fs::read(base.join(file)) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                out.push(fail(
                    format!("cannot read {field} \"{file}\": {}", file_reason(&e)),
                    "paths are relative to the config file; check the path and its permissions",
                ));
                None
            }
        };
    let (cert, key) = (read("cert", pair.cert, out), read("key", pair.key, out));
    let (cert, key) = (cert?, key?);
    let pem = "use a PEM certificate chain, like fullchain.pem, and its PEM private key";
    let chain = parse_certs(&cert)
        .map_err(|e| out.push(fail(format!("{} {e}", pair.cert), pem)))
        .ok()?;
    let private = parse_key(&key)
        .map_err(|e| out.push(fail(format!("{} {e}", pair.key), pem)))
        .ok()?;
    certified(chain, private)
        .map_err(|_| {
            out.push(fail(
                format!("{} does not fit {}", pair.key, pair.cert),
                "use the private key that was made with this certificate",
            ));
        })
        .ok()
}

fn unlisted(ck: &CertifiedKey, hosts: &[String]) -> Vec<String> {
    let listed: Vec<String> = hosts
        .iter()
        .filter_map(|h| host_name(h, SingleLabel::Allowed).ok())
        .collect();
    dns_names(ck)
        .into_iter()
        .filter(|n| !listed.iter().any(|l| burrow_syntax::web::covers(l, n)))
        .collect()
}

fn not_covered(ck: &CertifiedKey, hosts: &[String]) -> Vec<String> {
    hosts
        .iter()
        .filter_map(|h| host_name(h, SingleLabel::Allowed).ok())
        .filter(|h| !covers(ck, h))
        .collect()
}

pub(super) fn check_files(
    cfg: &ServerConfig,
    base: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let Some(web) = cfg.web.as_ref() else {
        return;
    };
    for (i, entry) in web.certificates.iter().enumerate() {
        let (Some(cert), Some(key)) = (entry.cert.as_deref(), entry.key.as_deref()) else {
            continue;
        };
        if cert.is_empty() || key.is_empty() {
            continue;
        }
        let pair = Pair {
            place: format!("web.certificates[{i}]"),
            at: spans.value(&["web", "certificates", &i.to_string()]),
            cert,
            key,
        };
        let Some(ck) = load(&pair, base, out) else {
            continue;
        };
        for host in not_covered(&ck, &entry.hosts) {
            out.push(
                Diagnostic::error(
                    "certificate-cover",
                    format!(
                        "{}: {} does not cover \"{host}\" listed in its hosts",
                        pair.place, pair.cert
                    ),
                )
                .at(pair.at)
                .help("remove the name from hosts, or use a certificate that covers it"),
            );
        }
        let extra = unlisted(&ck, &entry.hosts);
        if !extra.is_empty() {
            let names = extra
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(
                Diagnostic::warning(
                    "certificate-extra",
                    format!(
                        "{}: {} also covers {names}, which its hosts does not list; it is used only for the listed names",
                        pair.place, pair.cert
                    ),
                )
                .at(pair.at)
                .help("add the names to hosts if this file should serve them too"),
            );
        }
    }
    if web.https.is_none() {
        return;
    }
    for (name, svc) in &cfg.services {
        let Some(web @ ServiceWeb::Site(site)) = svc.web.as_ref() else {
            continue;
        };
        let Some(SiteTls::Files(files)) = site.tls.as_ref() else {
            continue;
        };
        let (Some(cert), Some(key)) = (files.cert.as_deref(), files.key.as_deref()) else {
            continue;
        };
        let pair = Pair {
            place: format!("services.{name}.web.tls"),
            at: spans.value(&["services", name, "web", "tls"]),
            cert,
            key,
        };
        let Some(ck) = load(&pair, base, out) else {
            continue;
        };
        for host in not_covered(&ck, web.hosts()) {
            out.push(
                Diagnostic::error(
                    "certificate-cover",
                    format!(
                        "{}: {} does not cover \"{host}\"; add the name to the certificate, move the name to another service, or list it in web.certificates",
                        pair.place, pair.cert
                    ),
                )
                .at(pair.at)
                .help("a site's own file is used for all of its names"),
            );
        }
    }
}
