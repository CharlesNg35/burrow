use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::OwnedSemaphorePermit;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, warn};

use super::control::control_session;
use super::{Shared, bind_error};
use crate::base::failure::Failure;
use crate::config::settings::WebSocketSettings;
use crate::wire::handshake::{AcceptError, accept};
use crate::wire::protocol::{ChannelKind, Hello, VERSION};
use crate::wire::secret::Secret;
use crate::wire::transport::tcp::{bind_tcp, tune};
use crate::wire::transport::ws::{self, Carrier};
use crate::wire::transport::{Conn, tls};

pub struct WsRoute {
    pub settings: WebSocketSettings,
    tls: Option<TlsAcceptor>,
}

impl WsRoute {
    pub fn new(settings: &WebSocketSettings) -> Result<WsRoute, Failure> {
        let tls = match &settings.tls {
            Some(m) => Some(tls::acceptor(&m.cert_pem, &m.key_pem).map_err(|e| {
                Failure::new(format!("websocket TLS files are not usable: {e}")).hint(format!(
                    "check websocket.tls.cert and websocket.tls.key with: {}",
                    crate::base::failure::cmd("validate")
                ))
            })?),
            None => None,
        };
        Ok(WsRoute {
            settings: settings.clone(),
            tls,
        })
    }
}

pub struct WsListener {
    pub listener: TcpListener,
    pub route: RwLock<Arc<WsRoute>>,
}

impl WsListener {
    pub fn bind(settings: &WebSocketSettings) -> Result<WsListener, Failure> {
        let route = WsRoute::new(settings)?;
        let listener = bind_tcp(settings.listen).map_err(|e| {
            bind_error(
                "the websocket listener",
                burrow_config::Protocol::Tcp,
                settings.listen,
                &e,
            )
        })?;
        Ok(WsListener {
            listener,
            route: RwLock::new(Arc::new(route)),
        })
    }

    pub fn route(&self) -> Arc<WsRoute> {
        self.route.read().expect("route lock").clone()
    }

    pub fn set_route(&self, route: WsRoute) {
        *self.route.write().expect("route lock") = Arc::new(route);
    }

    pub fn url_hint(&self) -> String {
        let route = self.route();
        let addr = self
            .listener
            .local_addr()
            .map_or_else(|_| route.settings.listen.to_string(), |a| a.to_string());
        let scheme = if route.tls.is_some() { "wss" } else { "ws" };
        format!("{scheme}://{addr}{}", route.settings.path)
    }
}

pub struct Permit {
    _slot: OwnedSemaphorePermit,
    _source: Option<burrow_syntax::share::Share<IpAddr>>,
}

pub fn try_permit(shared: &Arc<Shared>, peer: SocketAddr) -> Option<Permit> {
    let handshakes = shared.handshakes.read().expect("handshakes lock").clone();
    let total = shared.settings().max_handshakes;
    let used = total.saturating_sub(handshakes.available_permits());
    let source = match burrow_syntax::share::source_key(peer.ip()) {
        None => None,
        Some(key) => match shared.handshake_sources.take(key, total, used) {
            Some(share) => Some(share),
            None => {
                shared.refused_handshake();
                debug!(%peer, "too many unauthenticated connections from this address, dropping");
                return None;
            }
        },
    };
    match handshakes.try_acquire_owned() {
        Ok(p) => Some(Permit {
            _slot: p,
            _source: source,
        }),
        Err(_) => {
            shared.refused_handshake();
            debug!(%peer, "too many unauthenticated connections, dropping");
            None
        }
    }
}

pub async fn ws_accept_loop(shared: Arc<Shared>, ws: Arc<WsListener>) {
    loop {
        let (stream, peer) = match ws.listener.accept().await {
            Ok(v) => v,
            Err(e) if crate::wire::transport::tcp::gone_before_accept(&e) => {
                debug!("websocket: a connection was gone before it was accepted: {e}");
                continue;
            }
            Err(e) => {
                static FAILED: crate::policy::throttle::Throttle =
                    crate::policy::throttle::Throttle::new();
                if let Some(more) = FAILED.allow() {
                    warn!("websocket accept failed: {e}{more}");
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let Some(permit) = try_permit(&shared, peer) else {
            continue;
        };
        let shared = shared.clone();
        let route = ws.route();
        tokio::spawn(async move {
            handle_ws(shared, route, stream, peer, permit).await;
        });
    }
}

pub async fn handle_tcp(shared: Arc<Shared>, stream: TcpStream, peer: SocketAddr, permit: Permit) {
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
    ws: Arc<WsRoute>,
    stream: TcpStream,
    peer: SocketAddr,
    permit: Permit,
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
    let upgraded = tokio::time::timeout_at(deadline.into(), upgrade).await;
    upgraded_ws(
        shared,
        upgraded,
        ws.settings.keepalive,
        peer,
        permit,
        deadline,
    )
    .await;
}

async fn upgraded_ws(
    shared: Arc<Shared>,
    upgraded: Result<Result<ws::WsConn, ()>, tokio::time::error::Elapsed>,
    keepalive: std::time::Duration,
    peer: SocketAddr,
    permit: Permit,
    deadline: Instant,
) {
    match upgraded {
        Ok(Ok(mut conn)) => {
            conn.keep_alive(keepalive);
            authenticate(shared, Conn::from(conn), peer, permit, deadline).await
        }
        Ok(Err(())) => shared.refused_handshake(),
        Err(_) => {
            shared.refused_handshake();
            debug!(%peer, "websocket upgrade timed out");
        }
    }
}

#[cfg(feature = "web")]
pub async fn handle_web_tunnel(shared: Arc<Shared>, conn: burrow_web::TunnelConn<Permit>) {
    let burrow_web::TunnelConn {
        io,
        upgraded,
        path,
        peer,
        local,
        deadline,
        permit,
    } = conn;
    let Some(ws) = shared
        .settings()
        .websocket
        .clone()
        .filter(|w| w.host.is_some())
    else {
        return;
    };
    let carrier = Carrier::Accepted(Box::new(ws::Accepted {
        io: Box::new(io),
        local,
    }));
    let upgrade = async {
        if upgraded {
            return (path == ws.path)
                .then(|| ws::serve_upgraded(carrier))
                .ok_or(());
        }
        ws::accept(carrier, &ws.path).await.map_err(|e| {
            debug!(%peer, "websocket upgrade on the web proxy port refused: {e:?}");
        })
    };
    let upgraded = tokio::time::timeout_at(deadline, upgrade).await;
    upgraded_ws(
        shared,
        upgraded,
        ws.keepalive,
        peer,
        permit,
        deadline.into_std(),
    )
    .await;
}

fn secrets_for(shared: &Shared, kind: ChannelKind, name: &str) -> Vec<Secret> {
    let settings = shared.settings();
    let secrets = match kind {
        ChannelKind::Visit => settings.visitors.get(name).map(|v| &v.secrets),
        ChannelKind::Control | ChannelKind::Data => settings.clients.get(name).map(|c| &c.secrets),
    };
    secrets.cloned().unwrap_or_default()
}

async fn authenticate(
    shared: Arc<Shared>,
    conn: Conn,
    peer: SocketAddr,
    permit: Permit,
    deadline: Instant,
) {
    let settings = shared.settings();
    let result = tokio::time::timeout_at(
        deadline.into(),
        accept(conn, |k, n| secrets_for(&shared, k, n)),
    )
    .await;
    let acc = match result {
        Ok(Ok(a)) => a,
        failed => {
            shared.refused_handshake();
            match failed {
                Ok(Err(AcceptError::Rejected)) => {
                    debug!(%peer, "handshake rejected (wrong secret or unknown client name)")
                }
                Ok(Err(e)) => debug!(%peer, "handshake failed: {e:?}"),
                _ => debug!(%peer, "handshake timed out"),
            }
            return;
        }
    };
    let permit = (acc.preamble.kind == ChannelKind::Visit).then_some(permit);
    match (acc.preamble.kind, &acc.hello) {
        (ChannelKind::Control, &Hello::Control { version, instance }) if version == VERSION => {
            control_session(shared, acc, peer, instance).await
        }
        (ChannelKind::Data, &Hello::Data { version, token }) if version == VERSION => {
            if acc.preamble.plaintext != settings.insecure_plaintext {
                debug!(%peer, "data channel with another insecure_plaintext setting, closing");
                return;
            }
            match shared.session(&acc.preamble.name) {
                Some(s) if s.token == token => s.offer(acc.channel),
                _ => {
                    debug!(%peer, "data channel for an unknown session, closing");
                    let stale = [crate::wire::protocol::STALE_RECORD];
                    crate::forward::send_and_finish(acc.channel, &stale).await;
                }
            }
        }
        (ChannelKind::Visit, &Hello::Visit { version }) if version == VERSION => {
            super::visit::serve(shared, acc, peer, deadline, permit).await
        }
        (kind, hello) if kind != hello.kind() => {
            debug!(%peer, "hello does not match the channel kind, closing")
        }
        _ => debug!(%peer, "unsupported protocol version"),
    }
}
