use std::path::Path;

use super::Spans;
use super::fill_dns::FILE_PREFIX;
use crate::base::failure::file_reason;
use crate::config::diagnostics::Diagnostic;
use burrow_config::ServerConfig;

const CODE: &str = "dns-secret";

struct Field<'a> {
    path: [String; 6],
    value: &'a str,
    env: &'static str,
    inline_ok: bool,
}

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
            [
                "web".to_string(),
                "letsencrypt".into(),
                "dns".into(),
                i.to_string(),
                provider.into(),
                key.into(),
            ]
        };
        let mut fields = Vec::new();
        if let Some(c) = &e.cloudflare {
            fields.push(Field {
                path: at("cloudflare", "token"),
                value: &c.token,
                env: "CF_TOKEN",
                inline_ok: false,
            });
        }
        if let Some(r) = &e.rfc2136 {
            fields.push(Field {
                path: at("rfc2136", "secret"),
                value: &r.secret,
                env: "TSIG_SECRET",
                inline_ok: false,
            });
        }
        if let Some(a) = &e.acme_dns {
            fields.push(Field {
                path: at("acme_dns", "username"),
                value: &a.username,
                env: "ACMEDNS_USER",
                inline_ok: true,
            });
            fields.push(Field {
                path: at("acme_dns", "password"),
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

fn check(f: &Field<'_>, base_dir: &Path, spans: &Spans) -> Option<Diagnostic> {
    let path: Vec<&str> = f.path.iter().map(String::as_str).collect();
    let name = format!(
        "web.letsencrypt.dns[{}].{}.{}",
        f.path[3], f.path[4], f.path[5]
    );
    let at = spans.value(&path);
    let env = f.env;
    let (message, help) = match f.value.trim().strip_prefix(FILE_PREFIX) {
        Some(file) => {
            match std::fs::read_to_string(base_dir.join(file.trim())).map(zeroize::Zeroizing::new) {
                Err(e) => (
                    format!("{name}: cannot read {}: {}", file.trim(), file_reason(&e)),
                    "give a file burrow's user can read, relative to the config file".to_string(),
                ),
                Ok(text) if text.trim().is_empty() => (
                    format!("{name}: {} is empty", file.trim()),
                    "write the value into the file".to_string(),
                ),
                Ok(_) => return None,
            }
        }
        None if !f.inline_ok && !spans.from_env(&path) => (
            format!(
                "{name} is written in the file; use ${{{env}}} or file: so it stays out of the config"
            ),
            format!(
                "for example ${{{env}}}, set in the service's environment, or file:/etc/burrow/{}",
                env.to_ascii_lowercase()
            ),
        ),
        None if f.value.trim().is_empty() => (
            format!("{name} is empty"),
            format!("set it, for example from ${{{env}}}"),
        ),
        None => return None,
    };
    Some(Diagnostic::error(CODE, message).at(at).help(help))
}
