use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use http::{Request, Response};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use tracing::debug;

use super::idle::{Activity, Busy, Held, Watched};
use super::stall::Stalls;
use super::upstream::Upstreams;
use super::upstream::to_h2::StreamReset;
use super::{Body, pipeline};
use crate::Upstream;
use crate::io::BoxIo;
use crate::listen::Shared;

const LEAST: Duration = Duration::from_secs(1);
pub const DRAIN: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    H1,
    H2,
}

pub(crate) struct Conn {
    pub peer: SocketAddr,
    pub local: SocketAddr,
    pub name: Option<String>,
    pub proto: Proto,
    pub ups: Upstreams,
    pub activity: Arc<Activity>,
    pub stalls: Stalls,
}

impl Conn {
    pub(crate) fn new(
        peer: SocketAddr,
        local: SocketAddr,
        name: Option<String>,
        proto: Proto,
    ) -> Conn {
        Conn {
            peer,
            local,
            name,
            proto,
            ups: Upstreams::new(proto),
            activity: Activity::new(),
            stalls: Stalls::new(),
        }
    }
}

type Wrap = fn(&Conn, Body, Busy) -> Body;

fn held(_: &Conn, body: Body, busy: Busy) -> Body {
    Held::new(body, busy).boxed()
}

fn watched(conn: &Conn, body: Body, busy: Busy) -> Body {
    conn.stalls.watch(body, busy).boxed()
}

fn service<U: Upstream>(
    shared: &Arc<Shared<U>>,
    conn: &Arc<Conn>,
    wrap: Wrap,
) -> impl hyper::service::Service<
    Request<Incoming>,
    Response = Response<Body>,
    Error = h2::Error,
    Future: Send + 'static,
> + Clone
+ Send
+ 'static {
    let (shared, conn) = (shared.clone(), conn.clone());
    hyper::service::service_fn(move |req: Request<Incoming>| {
        let (shared, conn) = (shared.clone(), conn.clone());
        let busy = conn.activity.enter();
        async move {
            let version = req.version();
            let mut r = pipeline::handle(&shared, &conn, req).await;
            *r.version_mut() = version;
            if let Some(StreamReset(reason)) = r.extensions().get::<StreamReset>() {
                return Err(h2::Error::from(*reason));
            }
            Ok(r.map(|b| wrap(&conn, b, busy)))
        }
    })
}

pub(crate) async fn serve<U: Upstream>(shared: Arc<Shared<U>>, io: BoxIo, conn: Conn) {
    let proto = conn.proto;
    let conn = Arc::new(conn);
    let activity = conn.activity.clone();
    let io = TokioIo::new(Box::new(Watched::new(io, activity, proto == Proto::H1)) as BoxIo);
    match proto {
        Proto::H1 => {
            let served = super::h2::visitor_http1()
                .header_read_timeout(shared.timeouts().request_header.max(LEAST))
                .serve_connection(io, service(&shared, &conn, held))
                .with_upgrades();
            let served = std::pin::pin!(served);
            run(&shared, &conn, served, |s| s.graceful_shutdown(), false).await;
        }
        Proto::H2 => {
            let served = super::h2::server().serve_connection(io, service(&shared, &conn, watched));
            let served = std::pin::pin!(served);
            run(&shared, &conn, served, |s| s.graceful_shutdown(), true).await;
        }
    }
}

async fn run<U: Upstream, F: Future<Output = hyper::Result<()>>>(
    shared: &Shared<U>,
    conn: &Conn,
    mut served: Pin<&mut F>,
    shut: impl FnOnce(Pin<&mut F>),
    watch: bool,
) {
    let idle = shared.timeouts().idle;
    let mut stop = shared.stop();
    tokio::select! {
        r = served.as_mut() => {
            ended(conn, r);
            return;
        }
        () = conn.activity.idle(idle.max(LEAST)) => {
            debug!(peer = %conn.peer, "web: closing a connection idle for {idle:?}");
            return;
        }
        () = stalled(conn, watch) => {
            debug!(peer = %conn.peer, "web: closing a connection whose every response stalled");
            return;
        }
        _ = stop.wait_for(|s| *s) => {}
    }
    shut(served.as_mut());
    match tokio::time::timeout(DRAIN, served).await {
        Ok(r) => ended(conn, r),
        Err(_) => debug!(peer = %conn.peer, "web: a request outlasted the shutdown drain"),
    }
}

async fn stalled(conn: &Conn, watch: bool) {
    if watch {
        conn.stalls.all_stalled().await;
    } else {
        std::future::pending::<()>().await;
    }
}

fn ended(conn: &Conn, r: hyper::Result<()>) {
    if let Err(e) = r {
        debug!(peer = %conn.peer, "web: the connection ended: {e}");
    }
}
