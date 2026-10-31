//! In-memory byte pipes for the tunnel and the web proxy: each direction holds one fixed buffer only while data waits.
//! Depends on tokio and bytes only; never on burrow, burrow-web or the config and UI crates.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker, ready};

use bytes::{Buf, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Debug)]
struct Simplex {
    buf: BytesMut,
    max: usize,
    closed: bool,
    reader: Option<Waker>,
    writer: Option<Waker>,
}

impl Simplex {
    fn room(&mut self, want: usize) {
        if self.buf.capacity() - self.buf.len() < want {
            let mut fresh = BytesMut::with_capacity(self.max);
            fresh.extend_from_slice(&self.buf);
            self.buf = fresh;
        }
    }
}

type Shared = Arc<Mutex<Simplex>>;

fn simplex(max: usize) -> Shared {
    Arc::new(Mutex::new(Simplex {
        buf: BytesMut::new(),
        max: max.max(1),
        closed: false,
        reader: None,
        writer: None,
    }))
}

fn lock(s: &Shared) -> MutexGuard<'_, Simplex> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug)]
pub struct Duplex {
    read: Shared,
    write: Shared,
}

pub fn duplex(max: usize) -> (Duplex, Duplex) {
    let (one, two) = (simplex(max), simplex(max));
    (
        Duplex {
            read: one.clone(),
            write: two.clone(),
        },
        Duplex {
            read: two,
            write: one,
        },
    )
}

impl Duplex {
    fn close_write(&self) {
        let waker = {
            let mut s = lock(&self.write);
            s.closed = true;
            s.reader.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    fn close_read(&self) {
        let waker = {
            let mut s = lock(&self.read);
            s.closed = true;
            s.writer.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    fn put(&self, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        let coop = ready!(tokio::task::coop::poll_proceed(cx));
        let waker = {
            let mut s = lock(&self.write);
            if s.closed {
                return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
            }
            let avail = s.max - s.buf.len();
            if avail == 0 {
                s.writer = Some(cx.waker().clone());
                return Poll::Pending;
            }
            let want: usize = bufs.iter().map(|b| b.len()).sum::<usize>().min(avail);
            s.room(want);
            let mut left = want;
            for b in bufs {
                if left == 0 {
                    break;
                }
                let n = b.len().min(left);
                s.buf.extend_from_slice(&b[..n]);
                left -= n;
            }
            coop.made_progress();
            (s.reader.take(), want)
        };
        if let Some(w) = waker.0 {
            w.wake();
        }
        Poll::Ready(Ok(waker.1))
    }
}

impl Drop for Duplex {
    fn drop(&mut self) {
        self.close_write();
        self.close_read();
    }
}

impl AsyncRead for Duplex {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let coop = ready!(tokio::task::coop::poll_proceed(cx));
        let waker = {
            let mut s = lock(&self.read);
            if s.buf.has_remaining() {
                let n = s.buf.remaining().min(buf.remaining());
                buf.put_slice(&s.buf[..n]);
                s.buf.advance(n);
                if s.buf.is_empty() {
                    s.buf = BytesMut::new();
                }
                coop.made_progress();
                if n > 0 { s.writer.take() } else { None }
            } else if s.closed {
                coop.made_progress();
                None
            } else {
                s.reader = Some(cx.waker().clone());
                return Poll::Pending;
            }
        };
        if let Some(w) = waker {
            w.wake();
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Duplex {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.put(cx, &[io::IoSlice::new(buf)])
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.put(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.close_write();
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn bytes_cross_both_ways_and_a_drained_direction_holds_no_buffer() {
        let (mut a, mut b) = duplex(8);
        assert_eq!(a.write(b"0123456789").await.unwrap(), 8);
        assert_eq!(lock(&a.write).buf.capacity(), 8);
        let mut got = [0u8; 8];
        b.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"01234567");
        assert_eq!(lock(&b.read).buf.capacity(), 0);
        b.write_all(b"back").await.unwrap();
        let mut back = [0u8; 4];
        a.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"back");
    }

    #[tokio::test]
    async fn a_full_direction_waits_for_the_reader_and_never_grows_past_its_size() {
        let (mut a, mut b) = duplex(4);
        a.write_all(b"ab").await.unwrap();
        let mut two = [0u8; 1];
        b.read_exact(&mut two).await.unwrap();
        a.write_all(b"cd").await.unwrap();
        assert!(lock(&a.write).buf.capacity() <= 4);
        let mut one = [0u8; 1];
        b.read_exact(&mut one).await.unwrap();
        a.write_all(b"e").await.unwrap();
        assert!(lock(&a.write).buf.capacity() <= 4);
        a.write_all(b"f").await.unwrap();
        let w = tokio::spawn(async move {
            a.write_all(b"efgh").await.unwrap();
            a
        });
        let mut got = [0u8; 4];
        b.read_exact(&mut got).await.unwrap();
        assert_eq!(&two, b"a");
        assert_eq!(&one, b"b");
        assert_eq!(&got, b"cdef");
        drop(w.await.unwrap());
    }

    #[tokio::test]
    async fn shutdown_is_the_end_for_the_reader_and_drop_breaks_the_writer() {
        let (mut a, mut b) = duplex(16);
        a.write_all(b"x").await.unwrap();
        a.shutdown().await.unwrap();
        let mut got = Vec::new();
        b.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"x");
        drop(a);
        assert_eq!(
            b.write(b"late").await.unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[tokio::test]
    async fn a_vectored_write_takes_what_fits() {
        let (mut a, mut b) = duplex(6);
        let bufs = [io::IoSlice::new(b"head "), io::IoSlice::new(b"body")];
        assert_eq!(a.write_vectored(&bufs).await.unwrap(), 6);
        let mut got = [0u8; 6];
        b.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"head b");
    }
}
