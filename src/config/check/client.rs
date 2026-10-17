use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use super::super::model::{ClientConfig, OnRejected};
use super::super::settings::ClientSettings;
use super::super::tree::{Node, Value};
use super::server::insecure_warning;
use super::{Spans, name_error, secret_error, tuning, valid_name};
use crate::allowlist::{AllowList, Rule};
use crate::diagnostics::Diagnostic;
use crate::proxy::ProxyConfig;
use crate::secret::Secret;
use crate::suggest::{closest, quoted_list};
use crate::transport::endpoint::Endpoint;

const RESERVED_HEADERS: &[&str] = &[
    "host",
    "upgrade",
    "connection",
    "content-length",
    "transfer-encoding",
];

fn check_headers(
    cfg: &ClientConfig,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for (name, value) in &cfg.headers {
        let lower = name.to_ascii_lowercase();
        let reserved =
            RESERVED_HEADERS.contains(&lower.as_str()) || lower.starts_with("sec-websocket-");
        let valid_name = http::HeaderName::try_from(name.as_str()).is_ok();
        if reserved || !valid_name {
            out.push(
                Diagnostic::error("invalid-header", format!("header \"{name}\" cannot be set"))
                    .at(spans.key(&["headers", name]))
                    .label(if reserved { "set by burrow itself" } else { "not a valid header name" })
                    .help("use custom headers such as an access token, for example CF-Access-Client-Id"),
            );
            continue;
        }
        if http::HeaderValue::try_from(value.as_str()).is_err() {
            out.push(
                Diagnostic::error(
                    "invalid-header",
                    format!("header \"{name}\" has an invalid value"),
                )
                .at(spans.value(&["headers", name]))
                .without_snippet()
                .help("header values cannot contain line breaks or control characters"),
            );
            continue;
        }
        headers.push((name.clone(), value.clone()));
    }
    headers
}

pub fn check_client_refs(root: &Node, out: &mut Vec<Diagnostic>) {
    let declared: Vec<&str> = match root.get("clients").map(|c| &c.value) {
        Some(Value::Map(entries)) => entries.iter().map(|e| e.key.as_str()).collect(),
        _ => Vec::new(),
    };
    let Some(Value::Map(services)) = root.get("services").map(|s| &s.value) else {
        return;
    };
    for svc in services {
        let Some(client) = svc.value.get("client") else {
            continue;
        };
        let Some(name) = client.as_str() else {
            continue;
        };
        if declared.contains(&name) {
            continue;
        }
        let defined = if declared.is_empty() {
            "no clients are defined".to_string()
        } else {
            format!("defined clients: {}", quoted_list(declared.iter().copied()))
        };
        let help = match closest(name, declared.iter().copied()) {
            Some(s) => format!("did you mean \"{s}\"? {defined}"),
            None => format!("add it under \"clients:\"; {defined}"),
        };
        out.push(
            Diagnostic::error(
                "unknown-client",
                format!(
                    "unknown client \"{name}\" referenced by service \"{}\"",
                    svc.key
                ),
            )
            .at(client.span)
            .label("no client with this name")
            .help(help),
        );
    }
}

pub fn check_client(
    cfg: &ClientConfig,
    spans: &Spans,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) -> Option<ClientSettings> {
    let errors_before = out.iter().filter(|d| d.is_error()).count();

    let endpoint = match Endpoint::parse(&cfg.server) {
        Ok(e) => Some(e),
        Err(e) => {
            out.push(
                Diagnostic::error("invalid-address", format!("invalid server address: {e}"))
                    .at(spans.value(&["server"]))
                    .label("expected a host, host:port or wss://host/path")
                    .help("use a host name or IP (port 7835 unless you add :PORT), or the WebSocket URL (wss://tunnel.example.com/burrow)"),
            );
            None
        }
    };
    let endpoint = endpoint.filter(|_| spans.has(&["server"]));
    let websocket = endpoint.as_ref().is_some_and(Endpoint::is_websocket);
    let headers = check_headers(cfg, spans, out);
    if !cfg.headers.is_empty() && !websocket && endpoint.is_some() {
        out.push(
            Diagnostic::warning(
                "unused-option",
                "\"headers\" are only sent over WebSocket and are ignored for a TCP server address",
            )
            .at(spans.key(&["headers"]))
            .help("remove \"headers\", or use a ws:// or wss:// server address"),
        );
    }
    let tls = matches!(endpoint, Some(Endpoint::WebSocket { tls: true, .. }));
    let ca_pem = match &cfg.ca {
        None => None,
        Some(file) => {
            let path = base_dir.join(file);
            match std::fs::read(&path)
                .map_err(|e| crate::failure::file_reason(&e))
                .and_then(|pem| crate::transport::tls::parse_certs(&pem).map(|_| pem))
            {
                Ok(pem) => {
                    if !tls && endpoint.is_some() {
                        out.push(
                            Diagnostic::warning(
                                "unused-option",
                                "\"ca\" is only used for wss:// server addresses",
                            )
                            .at(spans.key(&["ca"]))
                            .help("remove \"ca\", or use a wss:// server address"),
                        );
                    }
                    Some(pem)
                }
                Err(e) => {
                    out.push(
                        Diagnostic::error(
                            "tls-file",
                            format!("cannot use ca file \"{file}\": {e}"),
                        )
                        .at(spans.value(&["ca"]))
                        .label("not a readable PEM certificate")
                        .help("paths are relative to the config file"),
                    );
                    None
                }
            }
        }
    };
    if cfg.insecure_plaintext && websocket {
        out.push(
            Diagnostic::warning(
                "unused-option",
                "insecure_plaintext does not apply over WebSocket: forwarded data stays encrypted",
            )
            .at(spans.value(&["insecure_plaintext"]))
            .help(
                "it must still match the server setting; remove it on both sides to silence this",
            ),
        );
    }
    if !valid_name(&cfg.name) {
        out.push(name_error("client", &cfg.name, spans.value(&["name"])));
    }
    let secret = match Secret::parse(&cfg.secret) {
        Ok(s) => Some(s),
        Err(e) => {
            out.push(secret_error(
                &format!("client \"{}\"", cfg.name),
                &e,
                spans.value(&["secret"]),
            ));
            None
        }
    };

    let mut rules = Vec::new();
    for (i, entry) in cfg.allow.iter().enumerate() {
        match entry.parse::<Rule>() {
            Ok(r) => rules.push(r),
            Err(e) => {
                let idx = i.to_string();
                let help = if entry.parse::<IpAddr>().is_err()
                    && !entry.contains('/')
                    && entry.chars().any(|c| c.is_ascii_alphabetic())
                    && !entry.contains("::")
                {
                    "allow entries are IP ranges, not host names; resolve the host and add its address, for example 192.168.1.10/32".to_string()
                } else {
                    "use a CIDR like 192.168.1.0/24, optionally with a port (192.168.1.0/24:22) or a range (:8000-8100)".to_string()
                };
                out.push(
                    Diagnostic::error(
                        "invalid-cidr",
                        format!("invalid allow entry: {}", e.describe()),
                    )
                    .at(spans.value(&["allow", &idx]))
                    .label("not a valid CIDR")
                    .help(help),
                );
            }
        }
    }
    if cfg.allow_all {
        out.push(
            Diagnostic::warning(
                "allow-all",
                "allow_all is enabled: the server can make this client connect anywhere",
            )
            .at(spans.value(&["allow_all"]))
            .label("no restriction on targets")
            .help("list only the networks you expose, for example allow: [127.0.0.0/8]"),
        );
        if !cfg.allow.is_empty() {
            out.push(
                Diagnostic::warning(
                    "allow-ignored",
                    "\"allow\" is ignored because allow_all is true",
                )
                .at(spans.key(&["allow"]))
                .help("remove allow_all to enforce the list"),
            );
        }
    } else if cfg.allow.is_empty() {
        let d = Diagnostic::warning(
            "no-allow-list",
            "no allow list: this client will refuse every target",
        )
        .help("add the networks the server may reach, for example: allow: [127.0.0.0/8]");
        out.push(if spans.has(&["allow"]) {
            d.at(spans.key(&["allow"]))
        } else {
            d
        });
    }

    let proxy = match &cfg.proxy {
        Some(p) => match ProxyConfig::parse(p) {
            Ok(p) => Some(p),
            Err(e) => {
                out.push(
                    Diagnostic::error("invalid-proxy", format!("invalid proxy: {e}"))
                        .at(spans.value(&["proxy"]))
                        .without_snippet()
                        .help("use http://host:port or socks5://[user:pass@]host:port"),
                );
                None
            }
        },
        None => None,
    };

    if cfg.insecure_plaintext && !websocket {
        out.push(insecure_warning(spans));
    }

    let initial = Duration::from_millis(cfg.reconnect.initial_delay_ms.into());
    let max = Duration::from_secs(cfg.reconnect.max_delay_secs.into());
    if initial > max {
        out.push(
            Diagnostic::error(
                "invalid-reconnect",
                "reconnect.initial_delay_ms is larger than reconnect.max_delay_secs",
            )
            .at(spans.value(&["reconnect", "initial_delay_ms"]))
            .help("make the initial delay smaller than the maximum delay"),
        );
    }

    if out.iter().filter(|d| d.is_error()).count() > errors_before {
        return None;
    }
    let endpoint = endpoint?;
    Some(ClientSettings {
        server: endpoint.to_string(),
        endpoint,
        headers,
        ca_pem,
        name: cfg.name.clone(),
        secret: secret?,
        allow: if cfg.allow_all {
            AllowList::All
        } else {
            AllowList::Rules(rules)
        },
        insecure_plaintext: cfg.insecure_plaintext,
        cipher: cfg.cipher,
        proxy,
        reconnect_initial: initial,
        reconnect_max: max,
        give_up_after: (cfg.reconnect.give_up_after_secs > 0)
            .then(|| Duration::from_secs(cfg.reconnect.give_up_after_secs.into())),
        exit_on_rejected: cfg.reconnect.on_rejected == OnRejected::Exit,
        reject_confirm: Duration::from_secs(15),
        connect_timeout: Duration::from_secs(cfg.connect_timeout_secs.into()),
        tcp: tuning(&cfg.tcp),
        udp_session_timeout: Duration::from_secs(cfg.udp.session_timeout_secs.into()),
    })
}
