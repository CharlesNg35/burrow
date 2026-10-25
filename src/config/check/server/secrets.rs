use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use crate::config::check::{Spans, binds_conflict, parse_bind, secret_error};
use crate::config::diagnostics::Diagnostic;
use crate::config::settings::{TlsMaterial, WebSocketSettings};
use crate::wire::noise::MAX_SECRETS_PER_CLIENT;
use crate::wire::secret::Secret;
use burrow_config::{Secrets, WebSocketConfig};

pub(super) fn check_secrets(
    kind: &str,
    name: &str,
    secret: &Secrets,
    spans: &Spans,
    seen: &mut Vec<([u8; 32], String)>,
    out: &mut Vec<Diagnostic>,
) -> Vec<Secret> {
    let section = match kind {
        "client" => "clients",
        _ => "visitors",
    };
    let owner = format!("{kind} \"{name}\"");
    let list = secret.as_slice();
    if list.is_empty() {
        out.push(
            Diagnostic::error(
                "missing-secret",
                format!("{owner} has an empty secret list"),
            )
            .at(spans.value(&[section, name, "secret"]))
            .help(format!(
                "add at least one secret; generate one with: {}",
                crate::base::failure::cmd("gensecret")
            )),
        );
    }
    if list.len() > MAX_SECRETS_PER_CLIENT {
        out.push(
            Diagnostic::error(
                "too-many-secrets",
                format!(
                    "{owner} has {} secrets, the limit is {MAX_SECRETS_PER_CLIENT}",
                    list.len()
                ),
            )
            .at(spans.value(&[section, name, "secret"]))
            .without_snippet()
            .help("keep only the current and the next secret while rotating"),
        );
    }
    let mut secrets = Vec::new();
    for (i, text) in list.iter().enumerate() {
        let idx = i.to_string();
        let span = match secret {
            Secrets::One(_) => spans.value(&[section, name, "secret"]),
            Secrets::Many(_) => spans.value(&[section, name, "secret", &idx]),
        };
        match Secret::parse(text) {
            Ok(s) => {
                if let Some((_, other)) = seen.iter().find(|(b, _)| b == s.as_bytes()) {
                    let msg = if *other == owner {
                        format!("{owner} lists the same secret twice")
                    } else {
                        format!("{owner} uses the same secret as {other}")
                    };
                    let who = if kind == "client" && other.starts_with("client ") {
                        "client"
                    } else {
                        "client and visitor"
                    };
                    out.push(
                        Diagnostic::error("duplicate-secret", msg)
                            .at(span)
                            .without_snippet()
                            .help(format!(
                                "every {who} needs its own secret; generate one with: {}",
                                crate::base::failure::cmd("gensecret")
                            )),
                    );
                }
                seen.push((*s.as_bytes(), owner.clone()));
                secrets.push(s);
            }
            Err(e) => out.push(secret_error(&owner, &e, span)),
        }
    }
    secrets
}

pub(super) fn check_websocket(
    ws: &WebSocketConfig,
    listen: Option<SocketAddr>,
    spans: &Spans,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) -> Option<WebSocketSettings> {
    let errors_before = out.iter().filter(|d| d.is_error()).count();
    let addr = match parse_bind(&ws.listen) {
        Ok(a) => Some(a),
        Err(_) => {
            out.push(
                Diagnostic::error(
                    "invalid-address",
                    format!("invalid websocket listen address \"{}\"", ws.listen),
                )
                .at(spans.value(&["websocket", "listen"]))
                .label("expected ip:port or a port")
                .help(format!("use an IP and port, for example 127.0.0.1:{} behind a proxy, or a port alone for 0.0.0.0", burrow_syntax::ports::WS_PORT)),
            );
            None
        }
    };
    if let (Some(a), Some(l)) = (addr, listen)
        && binds_conflict(a, l)
    {
        out.push(
            Diagnostic::error(
                burrow_admin_proto::codes::DUPLICATE_BIND,
                format!("websocket.listen {a} is the same address as listen"),
            )
            .at(spans.value(&["websocket", "listen"]))
            .label("already used by \"listen\"")
            .help("use a different port for the websocket listener"),
        );
    }
    if !burrow_syntax::endpoint::valid_path(&ws.path) {
        out.push(
            Diagnostic::error(
                "invalid-path",
                format!("invalid websocket path \"{}\"", ws.path),
            )
            .at(spans.value(&["websocket", "path"]))
            .label("expected a path like /burrow")
            .help("start the path with \"/\" and use letters, digits and - . _ ~ / only, at most 1024 of them"),
        );
    }
    let tls = ws.tls.as_ref().and_then(|t| {
        let read = |file: &str, field: &'static str, out: &mut Vec<Diagnostic>| {
            let path = base_dir.join(file);
            match std::fs::read(&path) {
                Ok(bytes) => Some((path, bytes)),
                Err(e) => {
                    out.push(
                        Diagnostic::error(crate::config::diagnostics::TLS_UNREADABLE, format!("cannot read websocket.tls.{field} \"{file}\": {}", crate::base::failure::file_reason(&e)))
                            .at(spans.value(&["websocket", "tls", field]))
                            .label("file not readable")
                            .help("paths are relative to the config file; check the path and its permissions"),
                    );
                    None
                }
            }
        };
        let cert = read(&t.cert, "cert", out);
        let key = read(&t.key, "key", out);
        let ((cert_path, cert_pem), (key_path, key_pem)) = (cert?, key?);
        if let Err(e) = crate::wire::transport::tls::acceptor(&cert_pem, &key_pem) {
            out.push(
                Diagnostic::error("tls-file", format!("websocket TLS files are not usable: {e}"))
                    .at(spans.value(&["websocket", "tls"]))
                    .without_snippet()
                    .help("use a PEM certificate chain and its matching PEM private key"),
            );
            return None;
        }
        Some(TlsMaterial {
            cert_path,
            key_path,
            cert_pem,
            key_pem: zeroize::Zeroizing::new(key_pem),
        })
    });
    if out.iter().filter(|d| d.is_error()).count() > errors_before {
        return None;
    }
    Some(WebSocketSettings {
        listen: addr?,
        path: ws.path.clone(),
        tls,
        keepalive: Duration::from_secs(ws.keepalive_secs.into()),
        host: None,
        shares: crate::config::settings::Shares::Nothing,
    })
}
