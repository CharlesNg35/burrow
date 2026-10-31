use std::net::SocketAddr;

use super::{BUILT_WITH_WEB, certs, challenge, on_terminated_site, plain_on_443, shares_port};
use crate::config::check::Spans;
use crate::config::diagnostics::Diagnostic;
use crate::config::settings::{Shares, WebSocketSettings};
use burrow_config::ServerConfig;
use burrow_syntax::web::{SingleLabel, host_name, is_wildcard};

fn host_unused(cfg: &ServerConfig) -> Diagnostic {
    if cfg.web.is_none() {
        return Diagnostic::error(
            "tunnel-host",
            "websocket.host is only used when the tunnel shares a web port, and this server has no web: block; remove websocket.host, or add web: {https: 443} (and web.letsencrypt for an automatic certificate) to share 443",
        )
        .help("for example web: {https: 443, letsencrypt: {email: you@example.com}} with websocket: {listen: 443, host: tunnel.example.com}");
    }
    Diagnostic::error(
        "tunnel-host",
        "websocket.host is only used when websocket.listen is the same port as web.https or web.http; remove it, or set websocket.listen: 443 (or 80)",
    )
    .help("for example web: {https: 443} with websocket: {listen: 443, host: tunnel.example.com}")
}

pub(in crate::config::check) fn share_tunnel(
    cfg: &ServerConfig,
    ports: (Option<SocketAddr>, Option<SocketAddr>),
    websocket: Option<&mut WebSocketSettings>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let (true, Some(ws), Some(given)) = (BUILT_WITH_WEB, websocket, cfg.websocket.as_ref()) else {
        return;
    };
    let host_at = spans.value(&["websocket", "host"]);
    let Ok(shares) = shares_port(cfg, ports, ws, &given.listen, spans, out) else {
        return;
    };
    ws.shares = shares;
    if shares == Shares::Nothing {
        out.extend(plain_on_443(cfg, ws, spans));
    }
    let Some(text) = given.host.as_deref() else {
        if shares != Shares::Nothing {
            let key = if shares == Shares::Https {
                "https"
            } else {
                "http"
            };
            out.push(
                Diagnostic::error(
                    "tunnel-host",
                    format!(
                        "websocket.host is required when websocket.listen is the same port as web.{key} ({}): it is the name that leads to the tunnel",
                        ws.listen.port()
                    ),
                )
                .at(spans.key(&["websocket"]))
                .help("add host: with the tunnel's name, like host: tunnel.example.com"),
            );
        }
        return;
    };
    if shares == Shares::Nothing {
        out.push(host_unused(cfg).at(host_at));
        return;
    }
    let host = match host_name(text, SingleLabel::Refused) {
        Ok(h) if is_wildcard(&h) => {
            out.push(
                Diagnostic::error(
                    "invalid-host",
                    format!("websocket.host \"{text}\" is a wildcard; the tunnel needs one name"),
                )
                .at(host_at)
                .help("write one name, like tunnel.example.com"),
            );
            return;
        }
        Ok(h) => h,
        Err(e) => {
            out.push(
                Diagnostic::error("invalid-host", format!("websocket.host: {e}"))
                    .at(host_at)
                    .help("write one name, like tunnel.example.com"),
            );
            return;
        }
    };
    let by_site = on_terminated_site(cfg, &host);
    if shares == Shares::Https && !by_site {
        certs::tunnel_has_source(cfg, (&host, ws.tls.is_some()), ports, spans, out);
    } else if given.challenge.is_some() {
        out.push(challenge::unused_tunnel(spans, by_site));
    }
    ws.host = Some(host);
}
