use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use tracing::debug;

use super::idle::{Activity, Watched};
use super::upstream::Upstreams;
use super::{MAX_HEAD, MAX_HEADERS, pipeline};
use crate::Upstream;
use crate::io::BoxIo;
use crate::listen::Shared;

const LEAST: Duration = Duration::from_secs(1);
pub const DRAIN: Duration = Duration::from_secs(10);

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
    let activity = Activity::new();
    let io: BoxIo = Box::new(Watched::new(io, activity.clone()));
    let service = {
        let (shared, conn, activity) = (shared.clone(), conn.clone(), activity.clone());
        hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
            let (shared, conn) = (shared.clone(), conn.clone());
            let busy = activity.enter();
            async move {
                let r = pipeline::handle(&shared, &conn, req).await;
                drop(busy);
                Ok::<_, Infallible>(r)
            }
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
    let mut served = std::pin::pin!(served);
    let mut stop = shared.stop();
    tokio::select! {
        r = served.as_mut() => {
            ended(&conn, r);
            return;
        }
        () = activity.idle(timeouts.idle.max(LEAST)) => {
            debug!(peer = %conn.peer, "web: closing a connection idle for {:?}", timeouts.idle);
            return;
        }
        _ = stop.wait_for(|s| *s) => {}
    }
    served.as_mut().graceful_shutdown();
    match tokio::time::timeout(DRAIN, served).await {
        Ok(r) => ended(&conn, r),
        Err(_) => debug!(peer = %conn.peer, "web: a request outlasted the shutdown drain"),
    }
}

fn ended(conn: &Conn, r: hyper::Result<()>) {
    if let Err(e) = r {
        debug!(peer = %conn.peer, "web: the connection ended: {e}");
    }
}
