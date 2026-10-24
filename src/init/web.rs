use serde_json::{Value, json};

use super::{InitOptions, WebSocketInit};
use crate::base::failure::Failure;
use burrow_syntax::web::{SingleLabel, host_name, is_wildcard};

pub(super) const HTTPS_PORT: u16 = 443;
const TERMS: &str = "https://letsencrypt.org/repository/";

pub(super) struct Tunnel {
    pub(super) host: String,
    pub(super) email: String,
    pub(super) path: String,
}

pub(super) fn check_email(email: &str) -> Result<String, Failure> {
    let e = email.trim();
    match e.split_once('@') {
        Some((who, domain)) if !who.is_empty() && domain.contains('.') && !e.contains(' ') => {
            Ok(e.to_string())
        }
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
