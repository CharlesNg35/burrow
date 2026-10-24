use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Instant;

use crate::io::{BoxIo, Io};

pub(crate) struct Activity {
    start: Instant,
    last_ms: AtomicU64,
    busy: AtomicUsize,
    failed: AtomicBool,
}

pub(crate) struct Busy(Arc<Activity>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.touch();
        self.0.busy.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Activity {
    pub(crate) fn new() -> Arc<Activity> {
        Arc::new(Activity {
            start: Instant::now(),
            last_ms: AtomicU64::new(0),
            busy: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
        })
    }

    pub(crate) fn fail(&self) {
        self.failed.store(true, Ordering::Release);
    }

    fn touch(&self) {
        let ms = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last_ms.store(ms, Ordering::Release);
    }

    pub(crate) fn enter(self: &Arc<Self>) -> Busy {
        self.busy.fetch_add(1, Ordering::AcqRel);
        Busy(self.clone())
    }

    fn quiet_since(&self) -> Option<Instant> {
        (self.busy.load(Ordering::Acquire) == 0)
            .then(|| self.start + Duration::from_millis(self.last_ms.load(Ordering::Acquire)))
    }

    pub(crate) async fn idle(&self, limit: Duration) {
        loop {
            match self.quiet_since() {
                Some(since) if since.elapsed() >= limit => return,
                Some(since) => tokio::time::sleep_until(since + limit).await,
                None => tokio::time::sleep(limit).await,
            }
        }
    }
}

pub(crate) struct Watched {
    inner: BoxIo,
    activity: Arc<Activity>,
}

impl Watched {
    pub(crate) fn new(inner: BoxIo, activity: Arc<Activity>) -> Watched {
        activity.touch();
        Watched { inner, activity }
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        if self.activity.failed.load(Ordering::Acquire) {
            self.inner.reset_on_close();
        }
    }
}

impl Io for Watched {
    fn reset_on_close(&self) {
        self.inner.reset_on_close();
    }
}

impl AsyncRead for Watched {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let before = buf.filled().len();
        let r = Pin::new(&mut me.inner).poll_read(cx, buf);
        if buf.filled().len() > before {
            me.activity.touch();
        }
        r
    }
}

impl AsyncWrite for Watched {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        let r = Pin::new(&mut me.inner).poll_write(cx, buf);
        if matches!(r, Poll::Ready(Ok(n)) if n > 0) {
            me.activity.touch();
        }
        r
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test(start_paused = true)]
    async fn quiet_means_no_bytes_either_way_and_nothing_in_flight() {
        let activity = Activity::new();
        let (a, mut b) = crate::pipe();
        let mut w = Watched::new(Box::new(a), activity.clone());
        let limit = Duration::from_secs(60);
        tokio::time::advance(Duration::from_secs(50)).await;
        w.write_all(b"x").await.unwrap();
        let mut one = [0u8; 1];
        b.read_exact(&mut one).await.unwrap();
        tokio::time::advance(Duration::from_secs(50)).await;
        assert!(
            activity.quiet_since().unwrap().elapsed() < limit,
            "a write counts"
        );
        b.write_all(b"y").await.unwrap();
        w.read_exact(&mut one).await.unwrap();
        let busy = activity.enter();
        tokio::time::advance(Duration::from_secs(600)).await;
        assert_eq!(
            activity.quiet_since(),
            None,
            "a request in flight is never idle"
        );
        drop(busy);
        let started = Instant::now();
        activity.idle(limit).await;
        assert_eq!(
            started.elapsed(),
            limit,
            "the clock starts when the request ends"
        );
    }
}
