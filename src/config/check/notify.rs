use std::path::Path;

use super::Spans;
use super::secret_ref::{SecretField, check_secret_ref, read_secret};
use crate::config::diagnostics::Diagnostic;
use crate::config::settings::{EmailSettings, NotifyTarget, NotifyVia, SmtpSecurity};
use burrow_config::{EmailNotify, NotifyEntry, ServerConfig};

const DESTINATION: &str = "notify-destination";
const URL: &str = "notify-url";
const EMAIL: &str = "notify-email";
const SECRET: &str = "notify-secret";
const SMTP_PORT: u16 = 587;
const SMTPS_PORT: u16 = 465;

fn at(i: usize, rest: &[&str]) -> Vec<String> {
    let mut p = vec!["notify".to_string(), i.to_string()];
    p.extend(rest.iter().map(|s| s.to_string()));
    p
}

fn span(spans: &Spans, path: &[String]) -> crate::config::tree::Span {
    let p: Vec<&str> = path.iter().map(String::as_str).collect();
    spans.value(&p)
}

pub(super) fn check_notify(
    cfg: &ServerConfig,
    base_dir: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Vec<NotifyTarget> {
    if cfg.notify.is_empty() {
        return Vec::new();
    }
    if !cfg!(feature = "web") {
        out.push(
            Diagnostic::error(
                DESTINATION,
                "this build of burrow has no notifications, so notify: cannot be used",
            )
            .at(spans.value(&["notify"]))
            .help("use the standard burrow build, or remove notify:"),
        );
        return Vec::new();
    }
    cfg.notify
        .iter()
        .enumerate()
        .filter_map(|(i, e)| entry(i, e, base_dir, spans, out))
        .collect()
}

fn entry(
    i: usize,
    e: &NotifyEntry,
    base_dir: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<NotifyTarget> {
    let set = [e.webhook.is_some(), e.ntfy.is_some(), e.email.is_some()];
    if set.iter().filter(|b| **b).count() != 1 {
        out.push(
            Diagnostic::error(
                DESTINATION,
                format!("notify[{i}]: name exactly one of webhook, ntfy or email"),
            )
            .at(span(spans, &at(i, &[])))
            .help("each entry is one destination, like - ntfy: https://ntfy.sh/my-topic; add another entry for the next one"),
        );
        return None;
    }
    let via = match (&e.webhook, &e.ntfy, &e.email) {
        (Some(url), _, _) => NotifyVia::Webhook(web_url(i, "webhook", url, spans, out)?),
        (_, Some(url), _) => NotifyVia::Ntfy(web_url(i, "ntfy", url, spans, out)?),
        (_, _, Some(m)) => NotifyVia::Email(email(i, m, base_dir, spans, out)?),
        _ => return None,
    };
    Some(NotifyTarget {
        via,
        events: e.events.iter().copied().collect(),
    })
}

fn web_url(
    i: usize,
    key: &str,
    text: &str,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<String> {
    let url = text.trim();
    let problem = match url.parse::<http::Uri>() {
        Err(_) => Some(format!("notify[{i}].{key}: \"{url}\" is not a URL")),
        Ok(u) => url_problem(&u).map(|why| format!("notify[{i}].{key}: {why}")),
    };
    match problem {
        None => Some(url.to_string()),
        Some(message) => {
            out.push(
                Diagnostic::error(URL, message)
                    .at(span(spans, &at(i, &[key])))
                    .help(match key {
                        "ntfy" => "for example https://ntfy.sh/my-burrow-topic, or ?auth=... at the end for a topic with a login",
                        _ => "for example https://hooks.example.com/burrow; put a token in the path or the query, not in user:password@",
                    }),
            );
            None
        }
    }
}

fn url_problem(u: &http::Uri) -> Option<String> {
    let Some(authority) = u.authority().filter(|a| !a.host().is_empty()) else {
        return Some("the address has no host".into());
    };
    if authority.as_str().contains('@') {
        return Some("a user:password@ in the address is not supported".into());
    }
    match u.scheme_str() {
        Some("https") => None,
        Some("http") => plain_problem(authority.host()),
        _ => Some("the address must start with https://".into()),
    }
}

#[cfg(feature = "web")]
fn plain_problem(host: &str) -> Option<String> {
    let ip = host
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .ok()?;
    (!burrow_web::tls::acme::http::is_private(ip)).then(|| {
        format!("{host} is a public address; plain http:// is only for this machine or the LAN, so use https://")
    })
}

#[cfg(not(feature = "web"))]
fn plain_problem(_: &str) -> Option<String> {
    None
}

fn email(
    i: usize,
    m: &EmailNotify,
    base_dir: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<EmailSettings> {
    let mut bad = |key: &str, message: String, help: &str| {
        out.push(
            Diagnostic::error(EMAIL, format!("notify[{i}].email.{key}: {message}"))
                .at(span(spans, &at(i, &["email", key])))
                .help(help.to_string()),
        );
    };
    let server = match smtp_server(m.smtp.trim()) {
        Ok(s) => Some(s),
        Err(why) => {
            bad(
                "smtp",
                why,
                "for example smtp://alerts@example.com@mail.example.com:587, or smtps:// for port 465; the password goes in password: ${SMTP_PASS}",
            );
            None
        }
    };
    let to = m.to.trim();
    if !is_address(to) {
        bad(
            "to",
            format!("\"{to}\" is not an email address"),
            "for example ops@example.com",
        );
    }
    let user = server.as_ref().and_then(|s| s.user.clone());
    let from = m
        .from
        .as_deref()
        .map(str::trim)
        .or(user.as_deref().filter(|u| is_address(u)));
    match from {
        Some(f) if !is_address(f) => bad(
            "from",
            format!("\"{f}\" is not an email address"),
            "for example burrow@example.com",
        ),
        None if server.is_some() => bad(
            "from",
            "is needed because the mail server's user name is not an address".into(),
            "for example from: burrow@example.com",
        ),
        _ => {}
    }
    let password = password(i, m, user.is_some(), base_dir, spans, out)?;
    let server = server?;
    let from = from.filter(|f| is_address(f))?.to_string();
    is_address(to).then(|| EmailSettings {
        to: to.to_string(),
        from,
        host: server.host,
        port: server.port,
        security: server.security,
        user,
        password,
    })
}

fn password(
    i: usize,
    m: &EmailNotify,
    has_user: bool,
    base_dir: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<Option<crate::config::settings::SecretText>> {
    let Some(value) = m.password.as_deref() else {
        return Some(None);
    };
    let field = SecretField {
        path: at(i, &["email", "password"]),
        name: format!("notify[{i}].email.password"),
        value,
        env: "SMTP_PASS",
        inline_ok: false,
        code: SECRET,
    };
    if let Some(d) = check_secret_ref(&field, base_dir, spans) {
        out.push(d);
        return None;
    }
    if !has_user {
        out.push(
            Diagnostic::error(
                EMAIL,
                format!("notify[{i}].email: a password needs a user name in smtp"),
            )
            .at(span(spans, &at(i, &["email", "smtp"])))
            .help("write the account before the host, like smtp://alerts@example.com@mail.example.com:587"),
        );
        return None;
    }
    Some(read_secret(value, base_dir))
}

struct SmtpServer {
    host: String,
    port: u16,
    security: SmtpSecurity,
    user: Option<String>,
}

fn smtp_server(text: &str) -> Result<SmtpServer, String> {
    let (scheme, rest) = text
        .split_once("://")
        .ok_or_else(|| format!("\"{text}\" does not start with smtp:// or smtps://"))?;
    let rest = rest.trim_end_matches('/');
    let (user, hostport) = match rest.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, rest),
    };
    if user.is_some_and(|u| u.contains(':')) {
        return Err("the password does not go in the address".into());
    }
    let user = user
        .map(|u| {
            percent_encoding::percent_decode_str(u)
                .decode_utf8_lossy()
                .into_owned()
        })
        .filter(|u| !u.is_empty());
    let (security, default_port) = match scheme {
        "smtps" => (SmtpSecurity::Tls, SMTPS_PORT),
        "smtp" => (SmtpSecurity::StartTls, SMTP_PORT),
        _ => return Err(format!("\"{scheme}://\" is not smtp:// or smtps://")),
    };
    let full = match hostport.ends_with(']') || !hostport.contains(':') {
        true => format!("{hostport}:{default_port}"),
        false => hostport.to_string(),
    };
    let (host, port) = burrow_syntax::hostport::parse_host_port(&full)?;
    let host = host.trim_matches(['[', ']']).to_string();
    let local = burrow_syntax::address::is_localhost(&host)
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    let security = match security {
        SmtpSecurity::StartTls if local => SmtpSecurity::StartTlsIfOffered,
        s => s,
    };
    Ok(SmtpServer {
        host,
        port,
        security,
        user,
    })
}

fn is_address(text: &str) -> bool {
    let Some((local, domain)) = text.rsplit_once('@') else {
        return false;
    };
    !local.is_empty()
        && !local.contains(|c: char| c.is_whitespace() || c.is_control() || c == '@')
        && burrow_syntax::web::host_name(domain, burrow_syntax::web::SingleLabel::Allowed).is_ok()
}

#[cfg(test)]
mod tests;
