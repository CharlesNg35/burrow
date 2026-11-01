use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use hyper::body::{Body as HttpBody, Frame, SizeHint};

use super::{H1, Key, Pool, Slot, Stream, Ticket};
use crate::Lease;
use crate::http::{Body, BoxError};

pub(crate) enum Home {
    H1 {
        pool: Arc<Pool>,
        key: Key,
        client: Arc<str>,
        conn: H1,
        epoch: u64,
    },
    H2(Stream),
}

impl Home {
    pub(crate) fn private(&mut self, conn: u64) {
        if let Home::H1 { key, .. } = self {
            *key = key.private(conn);
        }
    }

    fn go(self, ended: bool) {
        match self {
            Home::H1 {
                pool,
                key,
                client,
                conn,
                epoch,
            } => {
                if ended && epoch == pool.epoch() {
                    pool.h1.put(key, client, conn);
                }
            }
            Home::H2(stream) => drop(stream),
        }
    }
}

pub(crate) struct Back {
    inner: Body,
    home: Option<Home>,
    ticket: Option<Ticket>,
    lease: Option<Lease>,
    slot: Option<Slot>,
}

impl Back {
    pub(crate) fn new(
        inner: Body,
        home: Home,
        (lease, ticket): (Lease, Ticket),
        slot: Option<Slot>,
    ) -> Back {
        let mut back = Back {
            inner,
            home: Some(home),
            ticket: Some(ticket),
            lease: Some(lease),
            slot,
        };
        if back.inner.is_end_stream() {
            back.done(true);
        }
        back
    }

    fn done(&mut self, ended: bool) {
        self.ticket = None;
        if let Some(home) = self.home.take() {
            home.go(ended);
        }
        self.lease = None;
        self.slot = None;
    }
}

impl Drop for Back {
    fn drop(&mut self) {
        self.done(false);
    }
}

impl HttpBody for Back {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        match &polled {
            Poll::Ready(None) => this.done(true),
            Poll::Ready(Some(Err(_))) => this.done(false),
            Poll::Ready(Some(Ok(_))) if this.inner.is_end_stream() => this.done(true),
            _ => {}
        }
        if let Some(t) = this.ticket.as_mut() {
            t.waiting();
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
