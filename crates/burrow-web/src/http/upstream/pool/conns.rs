use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use hyper::client::conn::{http1, http2};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::task::AbortHandle;

use super::REUSE_WAIT;
use crate::http::Body;
use crate::{BoxIo, Ending};

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
    pub read: Arc<AtomicU64>,
}

pub(crate) struct Counted {
    inner: BoxIo,
    read: Arc<AtomicU64>,
}

impl Counted {
    pub(crate) fn new(inner: BoxIo) -> (Counted, Arc<AtomicU64>) {
        let read = Arc::new(AtomicU64::new(0));
        let counted = Counted {
            inner,
            read: read.clone(),
        };
        (counted, read)
    }
}

impl AsyncRead for Counted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        let got = buf.filled().len().saturating_sub(before);
        if got > 0 {
            self.read
                .fetch_add(u64::try_from(got).unwrap_or(u64::MAX), Ordering::Relaxed);
        }
        polled
    }
}

impl AsyncWrite for Counted {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
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
