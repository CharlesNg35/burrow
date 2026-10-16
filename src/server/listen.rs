use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::OwnedSemaphorePermit;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, warn};

use super::control::control_session;
use super::{Shared, bind_error};
use crate::config::settings::WebSocketSettings;
use crate::failure::Failure;
use crate::handshake::{AcceptError, accept};
use crate::protocol::{Hello, VERSION};
use crate::secret::Secret;
use crate::transport::tcp::{bind_tcp, tune};
use crate::transport::ws::{self, Carrier};
use crate::transport::{Conn, tls};

pub struct WsListener {
    pub listener: TcpListener,
    pub settings: WebSocketSettings,
    tls: Option<TlsAcceptor>,
}

impl WsListener {
    pub fn bind(settings: &WebSocketSettings) -> Result<WsListener, Failure> {
        let tls = match &settings.tls {
            Some(m) => Some(tls::acceptor(&m.cert_pem, &m.key_pem).map_err(|e| {
                Failure::new(format!("websocket TLS files are not usable: {e}"))
                    .hint("check websocket.tls.cert and websocket.tls.key with: burrow validate")
            })?),
            None => None,
        };
        let listener = bind_tcp(settings.listen)
            .map_err(|e| bind_error("the websocket listener", settings.listen, &e))?;
        Ok(WsListener {
            listener,
            settings: settings.clone(),
            tls,
        })
    }

    pub fn url_hint(&self) -> String {
        let addr = self
            .listener
            .local_addr()
            .map_or_else(|_| self.settings.listen.to_string(), |a| a.to_string());
        let scheme = if self.tls.is_some() { "wss" } else { "ws" };
        format!("{scheme}://{addr}{}", self.settings.path)
    }
}

pub fn try_permit(shared: &Arc<Shared>, peer: SocketAddr) -> Option<OwnedSemaphorePermit> {
    match shared.handshakes.clone().try_acquire_owned() {
        Ok(p) => Some(p),
        Err(_) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "too many unauthenticated connections, dropping");
            None
        }
    }
}

pub async fn ws_accept_loop(shared: Arc<Shared>, ws: Arc<WsListener>) {
    loop {
        let (stream, peer) = match ws.listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!("websocket accept failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let Some(permit) = try_permit(&shared, peer) else {
            continue;
        };
        let shared = shared.clone();
        let ws = ws.clone();
        tokio::spawn(async move {
            handle_ws(shared, ws, stream, peer, permit).await;
        });
    }
}

pub async fn handle_tcp(
    shared: Arc<Shared>,
    stream: TcpStream,
    peer: SocketAddr,
    permit: OwnedSemaphorePermit,
) {
    let settings = shared.settings();
    tune(&stream, &settings.tcp);
    authenticate(
        shared,
        Conn::from(stream),
        peer,
        permit,
        Instant::now() + settings.handshake_timeout,
    )
    .await;
}

async fn handle_ws(
    shared: Arc<Shared>,
    ws: Arc<WsListener>,
    stream: TcpStream,
    peer: SocketAddr,
    permit: OwnedSemaphorePermit,
) {
    let settings = shared.settings();
    tune(&stream, &settings.tcp);
    let deadline = Instant::now() + settings.handshake_timeout;
    let upgrade = async {
        let carrier = match &ws.tls {
            Some(acceptor) => {
                Carrier::ServerTls(Box::new(acceptor.accept(stream).await.map_err(|e| {
                    debug!(%peer, "websocket TLS handshake failed: {e}");
                })?))
            }
            None => Carrier::Plain(stream),
        };
        ws::accept(carrier, &ws.settings.path).await.map_err(|e| {
            debug!(%peer, "websocket upgrade refused: {e:?}");
        })
    };
    match tokio::time::timeout_at(deadline.into(), upgrade).await {
        Ok(Ok(conn)) => authenticate(shared, Conn::from(conn), peer, permit, deadline).await,
        Ok(Err(())) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "websocket upgrade timed out");
        }
    }
}

fn secrets_for(shared: &Shared, name: &str) -> Vec<Secret> {
    shared
        .settings()
        .clients
        .get(name)
        .map(|c| c.secrets.clone())
        .unwrap_or_default()
}

async fn authenticate(
    shared: Arc<Shared>,
    conn: Conn,
    peer: SocketAddr,
    permit: OwnedSemaphorePermit,
    deadline: Instant,
) {
    let settings = shared.settings();
    let result =
        tokio::time::timeout_at(deadline.into(), accept(conn, |n| secrets_for(&shared, n))).await;
    drop(permit);
    let acc = match result {
        Ok(Ok(a)) => a,
        Ok(Err(AcceptError::Rejected)) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "handshake rejected (wrong secret or unknown client name)");
            return;
        }
        Ok(Err(e)) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "handshake failed: {e:?}");
            return;
        }
        Err(_) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "handshake timed out");
            return;
        }
    };
    match acc.hello {
        Hello::Control { version } if version == VERSION => {
            control_session(shared, acc, peer).await
        }
        Hello::Data { version, token } if version == VERSION => {
            if acc.preamble.plaintext != settings.insecure_plaintext {
                return;
            }
            match shared.session(&acc.preamble.name) {
                Some(s) if s.token == token => s.offer(acc.channel),
                _ => debug!(%peer, "data channel for an unknown session, closing"),
            }
        }
        _ => debug!(%peer, "unsupported protocol version"),
    }
}
