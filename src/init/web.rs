use serde_json::{Value, json};

use super::{InitOptions, WebSocketInit};
use crate::base::failure::Failure;
use crate::base::prompt::Prompter;
use burrow_syntax::web::{SingleLabel, host_name, is_email, is_wildcard};

pub(super) const HTTPS_PORT: u16 = 443;
const TERMS: &str = "https://letsencrypt.org/repository/";

pub(super) struct Tunnel {
    pub(super) host: String,
    pub(super) email: String,
    pub(super) path: String,
}

pub(super) fn check_email(email: &str) -> Result<String, Failure> {
    match email.trim() {
        e if is_email(e) => Ok(e.to_string()),
        _ => Err(
            Failure::usage(format!("--email \"{email}\" is not an email address"))
                .hint("give Let's Encrypt's contact, like --email ops@example.com"),
        ),
    }
}

pub(super) fn tunnel_choice(opts: &InitOptions) -> Result<Option<Tunnel>, Failure> {
    let (Some(ws), Some(email)) = (&opts.websocket, &opts.email) else {
        return Ok(None);
    };
    let WebSocketInit {
        listen,
        path,
        tls_cert,
        tls_key,
    } = ws;
    if listen.is_some() || tls_cert.is_some() || tls_key.is_some() {
        return Err(Failure::usage(
            "--email gives the tunnel an automatic certificate on port 443 shared with the web proxy",
        )
        .hint("leave out --ws-listen, --ws-tls-cert and --ws-tls-key, or leave out --email"));
    }
    let named = |s: &str| {
        host_name(s, SingleLabel::Refused)
            .ok()
            .filter(|h| !is_wildcard(h))
    };
    let Some(host) = opts.server.as_deref().and_then(named) else {
        return Err(Failure::usage("--ws with --email needs the tunnel's name")
            .hint("pass --server tunnel.example.com, the name clients connect to"));
    };
    let path = path.clone().unwrap_or_else(burrow_config::default_ws_path);
    if !burrow_syntax::endpoint::valid_path(&path) {
        return Err(
            Failure::usage(format!("invalid --ws-path \"{path}\"")).hint("use a path like /burrow")
        );
    }
    Ok(Some(Tunnel {
        host,
        email: check_email(email)?,
        path,
    }))
}

pub(super) fn tunnel_value(t: &Tunnel, client: &str, secret_text: &str, service: Value) -> Value {
    json!({
        "web": { "https": HTTPS_PORT, "letsencrypt": { "email": t.email } },
        "websocket": { "listen": HTTPS_PORT, "host": t.host, "path": t.path },
        "clients": { client: { "secret": secret_text } },
        "services": { "web": service }
    })
}

pub(super) fn tunnel_note(t: &Tunnel) -> String {
    format!(
        "the tunnel shares port {HTTPS_PORT} with the web proxy, and {} gets its certificate from Let's Encrypt; writing the email accepts Let's Encrypt's terms ({TERMS}). Point {} at this server and open port {HTTPS_PORT}. No TCP control port is opened",
        t.host, t.host
    )
}

#[derive(Debug, Clone, Default)]
pub struct WebInit {
    pub domain: String,
    pub client: Option<String>,
    pub target: Option<String>,
}

pub(super) struct Site {
    pub(super) domain: String,
    pub(super) email: String,
    pub(super) http: bool,
    pub(super) client: String,
    pub(super) target: String,
    pub(super) service: String,
}

const DEFAULT_TARGET: &str = "3000";

fn site_name(domain: &str) -> Result<String, Failure> {
    match host_name(domain, SingleLabel::Refused) {
        Ok(h) if !is_wildcard(&h) => Ok(h),
        Ok(_) => Err(Failure::usage(format!(
            "\"{domain}\" is a wildcard; Let's Encrypt gives wildcards only through DNS checks, which come in a later phase"
        ))
        .hint("give the site's own name, like blog.example.com")),
        Err(e) => Err(Failure::usage(format!("invalid website name: {e}"))),
    }
}

fn service_of(domain: &str) -> String {
    let first = domain.split('.').next().unwrap_or_default();
    if crate::config::check::valid_name(first) && !first.chars().all(|c| c.is_ascii_digit()) {
        first.to_string()
    } else {
        "site".to_string()
    }
}

fn site(
    domain: &str,
    email: &str,
    client: &str,
    target: Option<&str>,
    http: bool,
) -> Result<Site, Failure> {
    let domain = site_name(domain)?;
    Ok(Site {
        service: service_of(&domain),
        domain,
        email: check_email(email)?,
        http,
        client: client.to_string(),
        target: target.map_or(DEFAULT_TARGET, str::trim).to_string(),
    })
}

pub(super) fn site_choice(
    opts: &InitOptions,
    client: &str,
    prompt: &mut dyn Prompter,
) -> Result<Option<Site>, Failure> {
    if let Some(w) = &opts.web {
        let Some(email) = &opts.email else {
            return Err(Failure::usage("--web needs --email, Let's Encrypt's contact")
                .hint(format!("for example --web {} --email ops@example.com; giving it accepts Let's Encrypt's terms", w.domain)));
        };
        if let Some(c) = w.client.as_deref().filter(|c| *c != client) {
            return Err(Failure::usage(format!(
                "--web-client {c} is not the client this config creates (\"{client}\")"
            ))
            .hint(format!(
                "pass --client-name {c} too, or leave out --web-client"
            )));
        }
        return site(&w.domain, email, client, w.target.as_deref(), true).map(Some);
    }
    if !prompt.interactive() || opts.yes || opts.email.is_some() {
        return Ok(None);
    }
    if !prompt.confirm("Serve websites (HTTPS for your apps)?", false) {
        return Ok(None);
    }
    let domain = prompt
        .ask("The website's name, like blog.example.com")
        .unwrap_or_default();
    let email = prompt
        .ask(&format!(
            "Let's Encrypt contact email, like ops@example.com (giving it accepts Let's Encrypt's terms, {TERMS})"
        ))
        .unwrap_or_default();
    let http = prompt.confirm(
        "Open port 80 too, so http:// visitors are sent to https://?",
        true,
    );
    let target = prompt
        .ask(&format!(
            "The app's port on client \"{client}\", like 3000 or 127.0.0.1:8080 (default {DEFAULT_TARGET})"
        ))
        .filter(|t| !t.trim().is_empty());
    site(&domain, &email, client, target.as_deref(), http).map(Some)
}

pub(super) fn with_site(value: Value, s: &Site) -> Value {
    let Value::Object(map) = value else {
        return value;
    };
    let mut block = json!({ "https": HTTPS_PORT });
    if s.http {
        block["http"] = json!(80);
    }
    block["letsencrypt"] = json!({ "email": s.email });
    let mut out = serde_json::Map::new();
    for (key, v) in map {
        match key.as_str() {
            "web" => {}
            "websocket" | "clients" if !out.contains_key("web") => {
                out.insert("web".into(), block.clone());
                out.insert(key, v);
            }
            "services" => {
                let target = s
                    .target
                    .parse::<u16>()
                    .map_or_else(|_| json!(s.target), |p| json!(p));
                let site = json!({ "client": s.client, "target": target, "web": s.domain });
                out.insert(key, json!({ s.service.clone(): site }));
            }
            _ => {
                out.insert(key, v);
            }
        }
    }
    Value::Object(out)
}

pub(super) fn site_note(s: &Site) -> String {
    let ports = if s.http { "443 and 80" } else { "443" };
    format!(
        "{} gets its certificate from Let's Encrypt; writing the email accepts Let's Encrypt's terms ({TERMS}). Point {} at this server and open port {ports}",
        s.domain, s.domain
    )
}

pub(super) fn allow_for(target: &str) -> String {
    if let Ok(port) = target.parse::<u16>() {
        return format!("127.0.0.1/32:{port}");
    }
    match target.parse::<std::net::SocketAddr>() {
        Ok(a) if a.is_ipv4() => format!("{}/32:{}", a.ip(), a.port()),
        Ok(a) => format!("{}/128:{}", a.ip(), a.port()),
        Err(_) => "127.0.0.1/32:3000".to_string(),
    }
}
