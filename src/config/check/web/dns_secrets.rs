use std::path::Path;

use super::Spans;
use crate::config::check::secret_ref::{SecretField as Field, check_secret_ref as check};
use crate::config::diagnostics::Diagnostic;
use burrow_config::ServerConfig;

const CODE: &str = "dns-secret";

pub(super) fn check_dns_secrets(
    cfg: &ServerConfig,
    base_dir: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let Some(le) = cfg.web.as_ref().and_then(|w| w.letsencrypt.as_ref()) else {
        return;
    };
    for (i, e) in le.dns.iter().enumerate() {
        let at = |provider: &str, key: &str| {
            vec![
                "web".to_string(),
                "letsencrypt".into(),
                "dns".into(),
                i.to_string(),
                provider.into(),
                key.into(),
            ]
        };
        let name = |provider: &str, key: &str| format!("web.letsencrypt.dns[{i}].{provider}.{key}");
        let mut fields = Vec::new();
        if let Some(c) = &e.cloudflare {
            fields.push(Field {
                path: at("cloudflare", "token"),
                name: name("cloudflare", "token"),
                code: CODE,
                value: &c.token,
                env: "CF_TOKEN",
                inline_ok: false,
            });
        }
        if let Some(r) = &e.rfc2136 {
            fields.push(Field {
                path: at("rfc2136", "secret"),
                name: name("rfc2136", "secret"),
                code: CODE,
                value: &r.secret,
                env: "TSIG_SECRET",
                inline_ok: false,
            });
        }
        if let Some(a) = &e.acme_dns {
            fields.push(Field {
                path: at("acme_dns", "username"),
                name: name("acme_dns", "username"),
                code: CODE,
                value: &a.username,
                env: "ACMEDNS_USER",
                inline_ok: true,
            });
            fields.push(Field {
                path: at("acme_dns", "password"),
                name: name("acme_dns", "password"),
                code: CODE,
                value: &a.password,
                env: "ACMEDNS_KEY",
                inline_ok: false,
            });
        }
        for f in fields {
            if let Some(d) = check(&f, base_dir, spans) {
                out.push(d);
            }
        }
    }
}
