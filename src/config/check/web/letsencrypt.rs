use std::path::Path;

use super::Spans;
use crate::base::failure::file_reason;
use crate::base::suggest::closest;
use crate::config::diagnostics::Diagnostic;
use burrow_config::{Directory, ServerConfig};
use burrow_syntax::web::is_email;
use burrow_web::tls::acme::http::trusted_roots;

const DIRECTORY: &str = "write production, staging, or the https:// directory URL of an ACME server, like https://ca.internal/acme/directory";

fn https_url(text: &str) -> bool {
    text.parse::<http::Uri>()
        .is_ok_and(|u| u.scheme_str() == Some("https") && u.host().is_some_and(|h| !h.is_empty()))
}

pub(super) fn check_letsencrypt(
    cfg: &ServerConfig,
    base: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let Some(le) = cfg.web.as_ref().and_then(|w| w.letsencrypt.as_ref()) else {
        return;
    };
    if !is_email(&le.email) {
        out.push(
            Diagnostic::error(
                "letsencrypt-email",
                format!(
                    "web.letsencrypt.email \"{}\" is not an email address",
                    le.email.trim()
                ),
            )
            .at(spans.value(&["web", "letsencrypt", "email"]))
            .help("give Let's Encrypt's contact, like email: ops@example.com"),
        );
    }
    if let Some(Directory::Url(text)) = &le.directory
        && !https_url(text.trim())
    {
        let text = text.trim();
        let help = match closest(text, ["production", "staging"]) {
            Some(s) => format!("did you mean \"{s}\"? {DIRECTORY}"),
            None => DIRECTORY.to_string(),
        };
        out.push(
            Diagnostic::error(
                "letsencrypt-directory",
                format!(
                    "web.letsencrypt.directory \"{text}\" is not production, staging or an https:// URL"
                ),
            )
            .at(spans.value(&["web", "letsencrypt", "directory"]))
            .help(help),
        );
    }
    let Some(ca) = le.ca.as_deref() else {
        return;
    };
    let problem = match std::fs::read(base.join(ca)) {
        Err(e) => Some(format!("cannot read {ca}: {}", file_reason(&e))),
        Ok(pem) => trusted_roots(Some(&pem)).err().map(|e| format!("{ca} {e}")),
    };
    if let Some(problem) = problem {
        out.push(
            Diagnostic::error("letsencrypt-ca", format!("web.letsencrypt.ca: {problem}"))
                .at(spans.value(&["web", "letsencrypt", "ca"]))
                .help("give the PEM file of the CA that signs the ACME server's certificate; paths are relative to the config file"),
        );
    }
}
