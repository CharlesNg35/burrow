use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use hyper::body::{Body as HttpBody, Frame, SizeHint};
use hyper::client::conn::http1::SendRequest;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use crate::Ending;
use crate::http::Proto;
use crate::http::{Body, BoxError};

pub const APP_CONNECTIONS: usize = 8;
pub const IDLE_EXTRA: Duration = Duration::from_secs(120);
const REUSE_WAIT: Duration = Duration::from_secs(1);

pub(crate) struct Stream {
    pub service: String,
    pub client: IpAddr,
    pub tx: SendRequest<Body>,
    pub ending: Option<Ending>,
    pub idle_since: Instant,
    pub app_key: u64,
}

impl Stream {
    fn serves(&self, service: &str, client: IpAddr) -> bool {
        self.service == service && self.client == client
    }

    fn stale(&self, service: &str, app_key: u64) -> bool {
        self.tx.is_closed() || (self.service == service && self.app_key != app_key)
    }
}

#[derive(Clone)]
pub(crate) struct H2App {
    pub service: String,
    pub client: IpAddr,
    pub tx: hyper::client::conn::http2::SendRequest<Body>,
    pub ending: Option<Ending>,
    pub app_key: u64,
}

pub(crate) struct Upstreams {
    idle: Mutex<Vec<Stream>>,
    cap: usize,
    slots: Arc<Semaphore>,
    h2: Mutex<Option<H2App>>,
}

pub(crate) fn app_connections(proto: Proto) -> usize {
    match proto {
        Proto::H1 => APP_CONNECTIONS,
        Proto::H2 => usize::try_from(crate::http::h2::MAX_STREAMS).unwrap_or(APP_CONNECTIONS),
    }
}

impl Upstreams {
    pub(crate) fn new(proto: Proto) -> Upstreams {
        let cap = app_connections(proto);
        Upstreams {
            idle: Mutex::default(),
            cap,
            slots: Arc::new(Semaphore::new(cap)),
            h2: Mutex::default(),
        }
    }
}

pub(crate) struct Slot {
    _permit: OwnedSemaphorePermit,
}

impl Upstreams {
    pub(crate) async fn slot(&self, deadline: Instant) -> Option<Slot> {
        let wait = self.slots.clone().acquire_owned();
        match tokio::time::timeout_at(deadline, wait).await {
            Ok(Ok(p)) => Some(Slot { _permit: p }),
            _ => None,
        }
    }

    pub(crate) async fn reuse(
        &self,
        service: &str,
        client: IpAddr,
        app_key: u64,
    ) -> Option<Stream> {
        let mut s = {
            let mut idle = self.idle.lock().expect("upstream lock");
            idle.retain(|s| !s.stale(service, app_key));
            if let Some(i) = idle
                .iter()
                .rposition(|s| s.serves(service, client) && s.tx.is_ready())
            {
                return Some(idle.swap_remove(i));
            }
            if self.slots.available_permits() + 1 < self.cap {
                return None;
            }
            let i = idle.iter().rposition(|s| s.serves(service, client))?;
            idle.swap_remove(i)
        };
        let ready = tokio::time::timeout(REUSE_WAIT, s.tx.ready()).await;
        matches!(ready, Ok(Ok(()))).then_some(s)
    }

    pub(crate) fn shared_h2(&self, service: &str, client: IpAddr, app_key: u64) -> Option<H2App> {
        let held = self.h2.lock().expect("upstream lock");
        held.as_ref()
            .filter(|s| {
                s.service == service
                    && s.client == client
                    && s.app_key == app_key
                    && !s.tx.is_closed()
            })
            .cloned()
    }

    pub(crate) fn keep_h2(&self, s: H2App) {
        *self.h2.lock().expect("upstream lock") = Some(s);
    }

    pub(crate) fn keep(&self, mut s: Stream, crowded: impl FnOnce() -> bool) {
        let mut idle = self.idle.lock().expect("upstream lock");
        idle.retain(|s| !s.tx.is_closed());
        let most = if idle.len() < APP_CONNECTIONS || !crowded() {
            self.cap
        } else {
            APP_CONNECTIONS
        };
        while idle.len() >= most {
            idle.remove(0);
        }
        s.idle_since = Instant::now();
        idle.push(s);
    }

    pub(crate) fn expire(&self, crowded: impl Fn(&str) -> bool) {
        let mut idle = self.idle.lock().expect("upstream lock");
        idle.retain(|s| !s.tx.is_closed());
        let now = Instant::now();
        while idle.len() > APP_CONNECTIONS
            && (idle[0].idle_since + IDLE_EXTRA <= now || crowded(&idle[0].service))
        {
            idle.remove(0);
        }
    }
}

pub(crate) struct Slotted {
    inner: Body,
    _slot: Slot,
}

impl Slotted {
    pub(crate) fn new(inner: Body, slot: Slot) -> Slotted {
        Slotted { inner, _slot: slot }
    }
}

impl HttpBody for Slotted {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn stream(service: &str) -> (Stream, tokio::io::DuplexStream) {
        let (ours, app) = tokio::io::duplex(1024);
        let (tx, conn) = hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(ours))
            .await
            .unwrap();
        tokio::spawn(conn);
        let s = Stream {
            service: service.to_string(),
            client: IpAddr::from([192, 0, 2, 1]),
            tx,
            ending: None,
            idle_since: Instant::now(),
            app_key: 0,
        };
        (s, app)
    }

    async fn kept(ups: &Upstreams, n: usize, crowded: bool) -> Vec<tokio::io::DuplexStream> {
        let mut apps = Vec::new();
        for _ in 0..n {
            let (s, app) = stream("site").await;
            ups.keep(s, || crowded);
            apps.push(app);
        }
        apps
    }

    fn idle(ups: &Upstreams) -> usize {
        ups.idle.lock().unwrap().len()
    }

    #[tokio::test(start_paused = true)]
    async fn an_http2_visitor_keeps_as_many_idle_app_connections_as_its_streams_and_extras_expire()
    {
        let ups = Upstreams::new(Proto::H2);
        let cap = app_connections(Proto::H2);
        let _apps = kept(&ups, cap + 5, false).await;
        assert_eq!(idle(&ups), cap, "the oldest go once the stream cap is idle");
        tokio::time::advance(IDLE_EXTRA - Duration::from_secs(1)).await;
        ups.expire(|_| false);
        assert_eq!(idle(&ups), cap, "nothing expires before the idle time");
        tokio::time::advance(Duration::from_secs(1)).await;
        ups.expire(|_| false);
        assert_eq!(
            idle(&ups),
            APP_CONNECTIONS,
            "extras expire, the first 8 stay"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_pressed_service_keeps_only_8_idle_app_connections() {
        let ups = Upstreams::new(Proto::H2);
        let _apps = kept(&ups, 20, true).await;
        assert_eq!(
            idle(&ups),
            APP_CONNECTIONS,
            "a pressed service keeps no extras"
        );
        let _more = kept(&ups, 12, false).await;
        assert_eq!(idle(&ups), 20);
        ups.expire(|s| s == "site");
        assert_eq!(
            idle(&ups),
            APP_CONNECTIONS,
            "extras close at once when a quarter of the pool is used"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_http1_visitor_still_keeps_8() {
        let ups = Upstreams::new(Proto::H1);
        let _apps = kept(&ups, 12, false).await;
        assert_eq!(idle(&ups), APP_CONNECTIONS);
    }
}
