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
use crate::http::{Body, BoxError};

pub const APP_CONNECTIONS: usize = 8;
const REUSE_WAIT: Duration = Duration::from_secs(1);

pub(crate) struct Stream {
    pub service: String,
    pub client: IpAddr,
    pub tx: SendRequest<Body>,
    pub ending: Option<Ending>,
}

impl Stream {
    fn serves(&self, service: &str, client: IpAddr) -> bool {
        self.service == service && self.client == client
    }
}

#[derive(Clone)]
pub(crate) struct H2App {
    pub service: String,
    pub client: IpAddr,
    pub tx: hyper::client::conn::http2::SendRequest<Body>,
    pub ending: Option<Ending>,
}

pub(crate) struct Upstreams {
    idle: Mutex<Vec<Stream>>,
    slots: Arc<Semaphore>,
    h2: Mutex<Option<H2App>>,
}

impl Default for Upstreams {
    fn default() -> Upstreams {
        Upstreams {
            idle: Mutex::default(),
            slots: Arc::new(Semaphore::new(APP_CONNECTIONS)),
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

    pub(crate) async fn reuse(&self, service: &str, client: IpAddr) -> Option<Stream> {
        let mut s = {
            let mut idle = self.idle.lock().expect("upstream lock");
            idle.retain(|s| !s.tx.is_closed());
            if let Some(i) = idle
                .iter()
                .position(|s| s.serves(service, client) && s.tx.is_ready())
            {
                return Some(idle.swap_remove(i));
            }
            if self.slots.available_permits() + 1 < APP_CONNECTIONS {
                return None;
            }
            let i = idle.iter().rposition(|s| s.serves(service, client))?;
            idle.swap_remove(i)
        };
        let ready = tokio::time::timeout(REUSE_WAIT, s.tx.ready()).await;
        matches!(ready, Ok(Ok(()))).then_some(s)
    }

    pub(crate) fn shared_h2(&self, service: &str, client: IpAddr) -> Option<H2App> {
        let held = self.h2.lock().expect("upstream lock");
        held.as_ref()
            .filter(|s| s.service == service && s.client == client && !s.tx.is_closed())
            .cloned()
    }

    pub(crate) fn keep_h2(&self, s: H2App) {
        *self.h2.lock().expect("upstream lock") = Some(s);
    }

    pub(crate) fn keep(&self, s: Stream) {
        let mut idle = self.idle.lock().expect("upstream lock");
        idle.retain(|s| !s.tx.is_closed());
        if idle.len() >= APP_CONNECTIONS {
            idle.remove(0);
        }
        idle.push(s);
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
