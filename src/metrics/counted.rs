use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct Counted<'a, S> {
    inner: S,
    bytes: Option<&'a AtomicU64>,
}

impl<'a, S> Counted<'a, S> {
    pub fn new(inner: S, bytes: Option<&'a AtomicU64>) -> Self {
        Counted { inner, bytes }
    }

    fn add(&self, n: usize) {
        if let Some(b) = self.bytes {
            b.fetch_add(n as u64, Ordering::Relaxed);
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Counted<'_, S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let before = buf.filled().len();
        ready!(Pin::new(&mut me.inner).poll_read(cx, buf))?;
        me.add(buf.filled().len() - before);
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Counted<'_, S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        let n = ready!(Pin::new(&mut me.inner).poll_write(cx, data))?;
        me.add(n);
        Poll::Ready(Ok(n))
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

    #[tokio::test]
    async fn counts_both_directions() {
        let read = AtomicU64::new(0);
        let written = AtomicU64::new(0);
        let (a, b) = tokio::io::duplex(1 << 16);
        let mut w = Counted::new(a, Some(&written));
        w.write_all(&[1u8; 5000]).await.unwrap();
        drop(w);
        let mut r = Counted::new(b, Some(&read));
        let mut got = Vec::new();
        r.read_to_end(&mut got).await.unwrap();
        assert_eq!(written.load(Ordering::Relaxed), 5000);
        assert_eq!(read.load(Ordering::Relaxed), 5000);
    }
}
