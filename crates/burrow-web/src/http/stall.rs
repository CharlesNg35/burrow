use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use hyper::body::{Body as HttpBody, Frame, SizeHint};
use tokio::time::Instant;
use tracing::debug;

use super::idle::Busy;
use super::{Body, BoxError};

pub const STALL: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_secs(1);

pub(crate) struct Stalls {
    start: Instant,
    open: Mutex<Vec<Weak<Slot>>>,
}

struct Slot {
    handed_ms: AtomicU64,
    handed: AtomicBool,
    stalled: AtomicBool,
    inner: Mutex<Option<Body>>,
}

pub(crate) struct Watch {
    slot: Arc<Slot>,
    start: Instant,
    _busy: Busy,
}

#[derive(Debug)]
pub struct Stalled;

impl std::fmt::Display for Stalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no progress for {} s, the stream is closed",
            STALL.as_secs()
        )
    }
}

impl std::error::Error for Stalled {}

fn ms_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

impl Stalls {
    pub(crate) fn new() -> Stalls {
        Stalls {
            start: Instant::now(),
            open: Mutex::default(),
        }
    }

    pub(crate) fn watch(&self, body: Body, busy: Busy) -> Watch {
        let slot = Arc::new(Slot {
            handed_ms: AtomicU64::new(ms_since(self.start)),
            handed: AtomicBool::new(true),
            stalled: AtomicBool::new(false),
            inner: Mutex::new(Some(body)),
        });
        let mut open = self.open.lock().expect("stall lock");
        open.retain(|w| w.strong_count() > 0);
        open.push(Arc::downgrade(&slot));
        Watch {
            slot,
            start: self.start,
            _busy: busy,
        }
    }

    fn sweep(&self) -> bool {
        let now = ms_since(self.start);
        let limit = u64::try_from(STALL.as_millis()).unwrap_or(u64::MAX);
        let mut open = self.open.lock().expect("stall lock");
        open.retain(|w| w.strong_count() > 0);
        let mut stalled = 0;
        for slot in open.iter().filter_map(Weak::upgrade) {
            if !slot.stalled.load(Ordering::Acquire)
                && slot.handed.load(Ordering::Acquire)
                && now.saturating_sub(slot.handed_ms.load(Ordering::Acquire)) >= limit
            {
                slot.stalled.store(true, Ordering::Release);
                let app = slot.inner.lock().expect("stall lock").take();
                drop(app);
                debug!(
                    "web: an HTTP/2 stream made no progress for {STALL:?}, the stream is closed"
                );
            }
            if slot.stalled.load(Ordering::Acquire) {
                stalled += 1;
            }
        }
        !open.is_empty() && stalled == open.len()
    }

    pub(crate) async fn all_stalled(&self, mut each_tick: impl FnMut()) {
        loop {
            tokio::time::sleep(TICK).await;
            each_tick();
            if self.sweep() {
                return;
            }
        }
    }
}

impl HttpBody for Watch {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let slot = &self.slot;
        slot.handed.store(false, Ordering::Release);
        let mut inner = slot.inner.lock().expect("stall lock");
        let Some(body) = inner.as_mut() else {
            return Poll::Ready(Some(Err(Box::new(Stalled))));
        };
        let polled = Pin::new(body).poll_frame(cx);
        if matches!(&polled, Poll::Ready(Some(Ok(f))) if f.is_data()) {
            slot.handed_ms
                .store(ms_since(self.start), Ordering::Release);
            slot.handed.store(true, Ordering::Release);
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        let inner = self.slot.inner.lock().expect("stall lock");
        inner.as_ref().is_some_and(HttpBody::is_end_stream)
    }

    fn size_hint(&self) -> SizeHint {
        let inner = self.slot.inner.lock().expect("stall lock");
        inner
            .as_ref()
            .map_or_else(SizeHint::default, HttpBody::size_hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::idle::Activity;
    use http_body_util::BodyExt;

    struct Chunks(usize);

    impl HttpBody for Chunks {
        type Data = Bytes;
        type Error = BoxError;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
            if self.0 == 0 {
                return Poll::Pending;
            }
            self.0 -= 1;
            Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"x")))))
        }
    }

    fn poll(w: &mut Watch) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let waker = std::task::Waker::noop();
        Pin::new(w).poll_frame(&mut Context::from_waker(waker))
    }

    #[tokio::test(start_paused = true)]
    async fn a_response_hyper_stops_taking_is_cut_after_10_s_and_a_quiet_one_is_not() {
        let stalls = Stalls::new();
        let activity = Activity::new();
        let mut taken = stalls.watch(Chunks(1).boxed(), activity.enter());
        let mut quiet = stalls.watch(Chunks(0).boxed(), activity.enter());
        assert!(poll(&mut taken).is_ready());
        assert!(poll(&mut quiet).is_pending());
        tokio::time::advance(STALL - TICK).await;
        assert!(!stalls.sweep(), "not before 10 s");
        tokio::time::advance(TICK).await;
        assert!(
            !stalls.sweep(),
            "the quiet stream waits on the app, not on the visitor"
        );
        assert!(
            matches!(poll(&mut taken), Poll::Ready(Some(Err(_)))),
            "the stalled stream ends in an error, so hyper resets it"
        );
        assert!(poll(&mut quiet).is_pending());
    }

    #[tokio::test(start_paused = true)]
    async fn the_connection_ends_when_every_open_response_is_stalled() {
        let stalls = Stalls::new();
        let activity = Activity::new();
        let mut a = stalls.watch(Chunks(5).boxed(), activity.enter());
        let mut b = stalls.watch(Chunks(5).boxed(), activity.enter());
        assert!(poll(&mut a).is_ready());
        assert!(poll(&mut b).is_ready());
        let started = Instant::now();
        tokio::time::timeout(STALL * 2, stalls.all_stalled(|| {}))
            .await
            .expect("every stream stalled closes the connection");
        assert!(started.elapsed() >= STALL);
        drop((a, b));
        assert!(!stalls.sweep(), "no open stream is not a stall");
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_that_keeps_being_read_never_stalls() {
        let stalls = Stalls::new();
        let activity = Activity::new();
        let mut w = stalls.watch(Chunks(100).boxed(), activity.enter());
        for _ in 0..30 {
            assert!(poll(&mut w).is_ready());
            tokio::time::advance(TICK).await;
            assert!(!stalls.sweep());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_response_never_polled_because_the_window_stays_shut_is_cut_too() {
        let stalls = Stalls::new();
        let activity = Activity::new();
        let mut shut = stalls.watch(Chunks(1).boxed(), activity.enter());
        tokio::time::advance(STALL).await;
        assert!(stalls.sweep(), "its only stream stalled");
        assert!(matches!(poll(&mut shut), Poll::Ready(Some(Err(_)))));
    }
}
