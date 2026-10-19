use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::{Buf, Bytes};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_websockets::{CloseCode, Limits, Message, ServerBuilder};

use super::token::Token;
use crate::dialer::Dialer;
use crate::dialer::admin::{AdminOpenError, AdminRefusal};
use burrow_admin_proto::frame::{AdminOpen, MAX_MESSAGE};

pub const AUTH_WAIT: Duration = Duration::from_secs(5);
pub const CLOSE_WAIT: Duration = Duration::from_secs(1);
const SERVER_PING: Duration = Duration::from_secs(25);

pub struct Rewind {
    prefix: Bytes,
    inner: TcpStream,
}

impl AsyncRead for Rewind {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.prefix.has_remaining() {
            let n = self.prefix.len().min(buf.remaining());
            buf.put_slice(&self.prefix[..n]);
            self.prefix.advance(n);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Rewind {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

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

pub async fn serve(
    stream: TcpStream,
    head: Vec<u8>,
    token: Token,
    dialer: Arc<Dialer>,
    visitor: String,
    server: String,
) {
    let rewind = Rewind {
        prefix: Bytes::from(head),
        inner: stream,
    };
    let limits = Limits::default().max_payload_len(Some(MAX_MESSAGE + 1024));
    let Ok((_, mut ws)) = ServerBuilder::new().limits(limits).accept(rewind).await else {
        return;
    };
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
    if ws.send(bridge_event("open", "", "")).await.is_err() {
        tx.close().await;
        return;
    }
    let why = loop {
        tokio::select! {
            from_browser = tokio::time::timeout(SERVER_PING, ws.next()) => match from_browser {
                Err(_) => {
                    if tx.ping().await.is_err() {
                        break "the server connection broke";
                    }
                }
                Ok(None) | Ok(Some(Err(_))) => break "the browser closed",
                Ok(Some(Ok(m))) if m.is_close() => break "the browser closed",
                Ok(Some(Ok(m))) => {
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
            from_server = rx.recv() => match from_server {
                Ok(Some(m)) => {
                    let Ok(text) = String::from_utf8(m) else {
                        break "the server sent text that is not UTF-8";
                    };
                    if ws.send(Message::text(text)).await.is_err() {
                        break "the browser went away";
                    }
                }
                Ok(None) => break "the server closed the admin session",
                Err(_) => break "the server connection broke",
            },
        }
    };
    let _ = ws.send(bridge_event("server-closed", why, why)).await;
    let _ = ws.close().await;
    tx.close().await;
}
