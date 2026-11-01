use std::sync::atomic::{AtomicU64, Ordering};

use hyper::client::conn::{http1, http2};
use tokio::task::AbortHandle;

use super::REUSE_WAIT;
use crate::Ending;
use crate::http::Body;

pub(crate) trait AppConn: Send + 'static {
    fn is_closed(&self) -> bool;

    fn is_ready(&self) -> bool;

    fn ending(&self) -> Option<Ending>;

    fn close(&self);
}

pub(crate) struct H1 {
    pub tx: http1::SendRequest<Body>,
    pub ending: Option<Ending>,
    pub task: AbortHandle,
}

impl AppConn for H1 {
    fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    fn is_ready(&self) -> bool {
        self.tx.is_ready()
    }

    fn ending(&self) -> Option<Ending> {
        self.ending.clone()
    }

    fn close(&self) {
        self.task.abort();
    }
}

impl H1 {
    pub(crate) async fn ready(mut self) -> Option<H1> {
        if self.tx.is_ready() {
            return Some(self);
        }
        let ready = tokio::time::timeout(REUSE_WAIT, self.tx.ready()).await;
        matches!(ready, Ok(Ok(()))).then_some(self)
    }
}

#[derive(Clone)]
pub(crate) struct H2 {
    pub id: u64,
    pub tx: http2::SendRequest<Body>,
    pub ending: Option<Ending>,
    pub task: AbortHandle,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

impl H2 {
    pub(crate) fn new(
        tx: http2::SendRequest<Body>,
        ending: Option<Ending>,
        task: AbortHandle,
    ) -> H2 {
        H2 {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            tx,
            ending,
            task,
        }
    }
}

impl AppConn for H2 {
    fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    fn is_ready(&self) -> bool {
        self.tx.is_ready()
    }

    fn ending(&self) -> Option<Ending> {
        self.ending.clone()
    }

    fn close(&self) {
        self.task.abort();
    }
}
