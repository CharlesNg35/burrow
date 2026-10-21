use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_websockets::{CloseCode, Limits, Message, ServerBuilder};

use super::token::Token;
use crate::dialer::Dialer;
use crate::dialer::admin::{AdminOpenError, AdminRefusal, Heard};
use burrow_admin_proto::frame::{AdminOpen, MAX_MESSAGE};

pub const AUTH_WAIT: Duration = Duration::from_secs(5);
pub const CLOSE_WAIT: Duration = Duration::from_secs(1);

fn bridge_event(state: &str, reason: &str, message: &str) -> Message {
    let v = serde_json::json!({"ev": "bridge", "body": {"state": state, "reason": reason, "message": message}});
    Message::text(v.to_string())
}

pub fn explain(e: &AdminOpenError, visitor: &str, server: &str) -> (&'static str, String) {
    match e {
        AdminOpenError::Refused(AdminRefusal::Forbidden) => (
            "forbidden",
            format!("visitor \"{visitor}\" has no admin rights on {server}"),
        ),
        AdminOpenError::Refused(AdminRefusal::Busy) => (
            "busy",
            format!(
                "{server} already has the most admin sessions it allows for visitor \"{visitor}\"; close another admin tab"
            ),
        ),
        AdminOpenError::Refused(AdminRefusal::NoAdmin) => (
            "no-admin",
            format!("{server} was built without the admin interface"),
        ),
        AdminOpenError::Refused(AdminRefusal::Explained(t)) => ("refused", t.clone()),
        AdminOpenError::Refused(AdminRefusal::Other(code)) => (
            "refused",
            format!("{server} refused the admin session (status {code})"),
        ),
        AdminOpenError::Dial(crate::dialer::DialError::Rejected) => (
            "rejected",
            format!("{server} rejected visitor \"{visitor}\"; check the name and secret"),
        ),
        AdminOpenError::Dial(crate::dialer::DialError::Unreachable(w))
        | AdminOpenError::Dial(crate::dialer::DialError::Lost(w)) => ("unreachable", w.clone()),
        AdminOpenError::Lost(w) => ("unreachable", w.clone()),
    }
}

pub fn open_message() -> AdminOpen {
    AdminOpen {
        min: burrow_admin_proto::PROTOCOL_MIN,
        max: burrow_admin_proto::PROTOCOL_MAX,
        client: format!("burrow-admin-ui/{}", env!("CARGO_PKG_VERSION")),
        features: Vec::new(),
    }
}

pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    token: Token,
    dialer: Arc<Dialer>,
    visitor: String,
    server: String,
) {
    let limits = Limits::default().max_payload_len(Some(MAX_MESSAGE + 1024));
    let mut ws = ServerBuilder::new().limits(limits).serve(stream);
    let first = tokio::time::timeout(AUTH_WAIT, ws.next()).await;
    let authed = match &first {
        Ok(Some(Ok(m))) => m
            .as_text()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
            .and_then(|v| {
                v.get("auth")
                    .and_then(|a| a.as_str())
                    .map(|s| token.matches(s))
            })
            .unwrap_or(false),
        _ => false,
    };
    if !authed {
        let _ = tokio::time::timeout(CLOSE_WAIT, async {
            let _ = ws
                .send(Message::close(
                    Some(CloseCode::POLICY_VIOLATION),
                    "unauthorized",
                ))
                .await;
            let _ = ws.close().await;
        })
        .await;
        return;
    }
    let (mut rx, mut tx, _) = match dialer.open_admin(&open_message()).await {
        Ok(p) => p,
        Err(e) => {
            let (reason, message) = explain(&e, &visitor, &server);
            let _ = ws.send(bridge_event("refused", reason, &message)).await;
            let _ = ws.close().await;
            return;
        }
    };
    let open = serde_json::json!({"ev": "bridge", "body": {"state": "open", "reason": "", "message": "", "server": server}});
    if ws.send(Message::text(open.to_string())).await.is_err() {
        tx.close().await;
        return;
    }
    let (ping, silence) = crate::admin::transport::keepalive();
    let mut pings = tokio::time::interval_at(tokio::time::Instant::now() + ping, ping);
    pings.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heard = tokio::time::Instant::now();
    let why = loop {
        tokio::select! {
            from_browser = ws.next() => match from_browser {
                None | Some(Err(_)) => break "the browser closed",
                Some(Ok(m)) if m.is_close() => break "the browser closed",
                Some(Ok(m)) => {
                    let Some(text) = m.as_text() else {
                        continue;
                    };
                    if text.len() > MAX_MESSAGE {
                        break "a message from the browser is too large";
                    }
                    if tx.send(text.as_bytes()).await.is_err() {
                        break "the server connection broke";
                    }
                }
            },
            _ = pings.tick() => {
                if tx.ping().await.is_err() {
                    break "the server connection broke";
                }
            }
            from_server = rx.recv_record() => {
                heard = tokio::time::Instant::now();
                match from_server {
                    Ok(Some(Heard::Message(m))) => {
                        let Ok(text) = String::from_utf8(m) else {
                            break "the server sent text that is not UTF-8";
                        };
                        if ws.send(Message::text(text)).await.is_err() {
                            break "the browser went away";
                        }
                    }
                    Ok(Some(Heard::Record)) => {}
                    Ok(None) => break "the server closed the admin session",
                    Err(_) => break "the server connection broke",
                }
            }
            _ = tokio::time::sleep_until(heard + silence) => {
                break "the server sent nothing for 90 s";
            }
        }
    };
    let _ = ws.send(bridge_event("server-closed", why, why)).await;
    let _ = ws.close().await;
    tx.close().await;
}
