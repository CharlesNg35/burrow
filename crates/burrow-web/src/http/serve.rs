use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use tracing::debug;

use super::upstream::Upstreams;
use super::{MAX_HEAD, MAX_HEADERS, pipeline};
use crate::Upstream;
use crate::io::BoxIo;
use crate::listen::Shared;

const LEAST: Duration = Duration::from_secs(1);

pub(crate) struct Conn {
    pub peer: SocketAddr,
    pub local: SocketAddr,
    pub name: Option<String>,
    pub ups: Upstreams,
}

impl Conn {
    pub(crate) fn new(peer: SocketAddr, local: SocketAddr, name: Option<String>) -> Conn {
        Conn {
            peer,
            local,
            name,
            ups: Upstreams::default(),
        }
    }
}

pub(crate) async fn serve<U: Upstream>(shared: Arc<Shared<U>>, io: BoxIo, conn: Conn) {
    let conn = Arc::new(conn);
    let timeouts = shared.timeouts();
    let service = {
        let (shared, conn) = (shared.clone(), conn.clone());
        hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
            let (shared, conn) = (shared.clone(), conn.clone());
            async move { Ok::<_, Infallible>(pipeline::handle(&shared, &conn, req).await) }
        })
    };
    let served = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(timeouts.request_header.max(LEAST))
        .max_header_size(MAX_HEAD)
        .max_headers(MAX_HEADERS)
        .keep_alive(true)
        .half_close(false)
        .serve_connection(TokioIo::new(io), service)
        .with_upgrades();
    if let Err(e) = served.await {
        debug!(peer = %conn.peer, "web: the connection ended: {e}");
    }
}
