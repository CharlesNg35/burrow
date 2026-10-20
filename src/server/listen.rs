use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::OwnedSemaphorePermit;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, warn};

use super::control::control_session;
use super::{Shared, bind_error};
use crate::config::settings::WebSocketSettings;
use crate::failure::Failure;
use crate::handshake::{AcceptError, accept};
use crate::protocol::{ChannelKind, Hello, VERSION};
use crate::secret::Secret;
use crate::transport::tcp::{bind_tcp, tune};
use crate::transport::ws::{self, Carrier};
use crate::transport::{Conn, tls};

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
                    crate::failure::cmd("validate")
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
    _source: Option<SourceSlot>,
}

struct SourceSlot {
    sources: Sources,
    key: IpAddr,
}

impl Drop for SourceSlot {
    fn drop(&mut self) {
        let mut sources = self.sources.lock().expect("sources lock");
        if let Some(n) = sources.get_mut(&self.key) {
            *n -= 1;
            if *n == 0 {
                sources.remove(&self.key);
            }
        }
    }
}

fn source_key(ip: IpAddr) -> Option<IpAddr> {
    let ip = ip.to_canonical();
    if ip.is_loopback() {
        return None;
    }
    Some(match ip {
        IpAddr::V6(v6) => IpAddr::V6((v6.to_bits() >> 64 << 64).into()),
        v4 => v4,
    })
}

type Sources = Arc<std::sync::Mutex<HashMap<IpAddr, usize>>>;

fn source_slot(
    sources: &Sources,
    ip: IpAddr,
    total: usize,
    pressed: bool,
) -> Result<Option<SourceSlot>, ()> {
    let Some(key) = source_key(ip) else {
        return Ok(None);
    };
    let most = (total / 8).max(8);
    let mut held = sources.lock().expect("sources lock");
    let n = held.entry(key).or_insert(0);
    if pressed && *n >= most {
        return Err(());
    }
    *n += 1;
    Ok(Some(SourceSlot {
        sources: sources.clone(),
        key,
    }))
}

pub fn try_permit(shared: &Arc<Shared>, peer: SocketAddr) -> Option<Permit> {
    let handshakes = shared.handshakes.read().expect("handshakes lock").clone();
    let total = shared.settings().max_handshakes;
    let pressed = handshakes.available_permits() < total / 2;
    let source = match source_slot(&shared.handshake_sources, peer.ip(), total, pressed) {
        Ok(source) => source,
        Err(()) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            shared
                .metrics
                .handshakes_refused
                .fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "too many unauthenticated connections from this address, dropping");
            return None;
        }
    };
    match handshakes.try_acquire_owned() {
        Ok(p) => Some(Permit {
            _slot: p,
            _source: source,
        }),
        Err(_) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            shared
                .metrics
                .handshakes_refused
                .fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "too many unauthenticated connections, dropping");
            None
        }
    }
}

pub async fn ws_accept_loop(shared: Arc<Shared>, ws: Arc<WsListener>) {
    loop {
        let (stream, peer) = match ws.listener.accept().await {
            Ok(v) => v,
            Err(e) if crate::transport::tcp::gone_before_accept(&e) => {
                debug!("websocket: a connection was gone before it was accepted: {e}");
                continue;
            }
            Err(e) => {
                static FAILED: crate::throttle::Throttle = crate::throttle::Throttle::new();
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
    match tokio::time::timeout_at(deadline.into(), upgrade).await {
        Ok(Ok(mut conn)) => {
            conn.keep_alive(ws.settings.keepalive);
            authenticate(shared, Conn::from(conn), peer, permit, deadline).await
        }
        Ok(Err(())) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            shared
                .metrics
                .handshakes_refused
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            shared
                .metrics
                .handshakes_refused
                .fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "websocket upgrade timed out");
        }
    }
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
        Ok(Err(AcceptError::Rejected)) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            shared
                .metrics
                .handshakes_refused
                .fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "handshake rejected (wrong secret or unknown client name)");
            return;
        }
        Ok(Err(e)) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            shared
                .metrics
                .handshakes_refused
                .fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "handshake failed: {e:?}");
            return;
        }
        Err(_) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            shared
                .metrics
                .handshakes_refused
                .fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "handshake timed out");
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
                    let mut ch = acc.channel;
                    let _ = ch
                        .writer
                        .send(&mut ch.stream, &[crate::protocol::STALE_RECORD])
                        .await;
                    let _ = ch.stream.flush().await;
                    let _ = ch.stream.finish().await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_address_cannot_take_every_handshake_slot() {
        let sources = Sources::default();
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let calm: Vec<_> = (0..64)
            .map(|_| source_slot(&sources, ip, 256, false).unwrap())
            .collect();
        assert!(
            source_slot(&sources, ip, 256, true).is_err(),
            "a busy server refuses an address over its share"
        );
        assert!(source_slot(&sources, "203.0.113.8".parse().unwrap(), 256, true).is_ok());
        let v6 = |s: &str| s.parse::<IpAddr>().unwrap();
        let same_net: Vec<_> = (0..32)
            .map(|i| source_slot(&sources, v6(&format!("2001:db8::{i:x}")), 256, true).unwrap())
            .collect();
        assert!(source_slot(&sources, v6("2001:db8::ffff:1"), 256, true).is_err());
        assert!(
            source_slot(&sources, v6("::1"), 256, true)
                .unwrap()
                .is_none()
        );
        drop(calm);
        drop(same_net);
        assert!(
            sources.lock().unwrap().is_empty(),
            "every slot is given back"
        );
        assert!(source_slot(&sources, ip, 256, true).is_ok());
    }
}
