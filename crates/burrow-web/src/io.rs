use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::Notify;

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    fn reset_on_close(&self) {}

    fn ending(&self) -> Option<Ending> {
        None
    }
}

#[derive(Debug, Default)]
struct Side {
    reason: AtomicU8,
    via: AtomicU64,
    done: AtomicBool,
    ended: Notify,
}

#[derive(Debug, Clone)]
pub struct Ending(Arc<Side>);

impl Ending {
    pub fn refused(&self) -> Option<crate::Refused> {
        crate::Refused::from_code(self.0.reason.load(Ordering::Acquire))
    }

    pub fn answered_by(&self, way: u64) {
        self.0.via.store(way, Ordering::Release);
    }

    pub fn via(&self) -> Option<u64> {
        Some(self.0.via.load(Ordering::Acquire)).filter(|v| *v != 0)
    }

    pub fn is_ended(&self) -> bool {
        self.0.done.load(Ordering::Acquire)
    }

    pub fn end(&self) {
        self.0.done.store(true, Ordering::Release);
        self.0.ended.notify_waiters();
    }

    pub async fn ended(&self) {
        let notified = self.0.ended.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.0.done.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

pub type BoxIo = Box<dyn Io>;

impl Io for BoxIo {
    fn reset_on_close(&self) {
        (**self).reset_on_close();
    }

    fn ending(&self) -> Option<Ending> {
        (**self).ending()
    }
}

impl Io for TcpStream {
    fn reset_on_close(&self) {
        let _ = socket2::SockRef::from(self).set_linger(Some(Duration::ZERO));
    }
}

impl<S: Io> Io for tokio_rustls::server::TlsStream<S> {
    fn reset_on_close(&self) {
        self.get_ref().0.reset_on_close();
    }
}

impl<S: Io> Io for tokio_rustls::client::TlsStream<S> {
    fn reset_on_close(&self) {
        self.get_ref().0.reset_on_close();
    }

    fn ending(&self) -> Option<Ending> {
        self.get_ref().0.ending()
    }
}

pub struct Prefixed<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    pub fn new(prefix: Vec<u8>, inner: S) -> Self {
        Prefixed {
            prefix,
            pos: 0,
            inner,
        }
    }

    pub fn get_ref(&self) -> &S {
        &self.inner
    }

    pub fn into_parts(mut self) -> (Vec<u8>, S) {
        let left = self.prefix.split_off(self.pos.min(self.prefix.len()));
        (left, self.inner)
    }
}

impl<S: Io> Io for Prefixed<S> {
    fn reset_on_close(&self) {
        self.inner.reset_on_close();
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        if me.pos < me.prefix.len() {
            let n = (me.prefix.len() - me.pos).min(buf.remaining());
            buf.put_slice(&me.prefix[me.pos..me.pos + n]);
            me.pos += n;
            if me.pos == me.prefix.len() {
                me.prefix = Vec::new();
                me.pos = 0;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut me.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

pub const PIPE_BUFFER: usize = 32 * 1024;

const RESET: u8 = 1;
const DROPPED: u8 = 2;

pub struct Pipe {
    inner: DuplexStream,
    mine: Arc<AtomicU8>,
    theirs: Arc<AtomicU8>,
    side: Arc<Side>,
}

pub fn pipe() -> (Pipe, Pipe) {
    let (a, b) = tokio::io::duplex(PIPE_BUFFER);
    let (ra, rb) = (Arc::new(AtomicU8::new(0)), Arc::new(AtomicU8::new(0)));
    let side = Arc::new(Side::default());
    (
        Pipe {
            inner: a,
            mine: ra.clone(),
            theirs: rb.clone(),
            side: side.clone(),
        },
        Pipe {
            inner: b,
            mine: rb,
            theirs: ra,
            side,
        },
    )
}

impl Pipe {
    pub fn set_reset(&self, reset: bool) {
        if reset {
            self.mine.fetch_or(RESET, Ordering::AcqRel);
        } else {
            self.mine.fetch_and(!RESET, Ordering::AcqRel);
        }
    }

    pub fn refuse(&self, why: crate::Refused) {
        self.side.reason.store(why.code(), Ordering::Release);
    }

    fn peer_reset(&self) -> bool {
        self.theirs.load(Ordering::Acquire) == RESET | DROPPED
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        self.mine.fetch_or(DROPPED, Ordering::AcqRel);
    }
}

impl Io for Pipe {
    fn reset_on_close(&self) {
        self.set_reset(true);
    }

    fn ending(&self) -> Option<Ending> {
        Some(Ending(self.side.clone()))
    }
}

impl AsyncRead for Pipe {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let before = buf.filled().len();
        ready!(Pin::new(&mut me.inner).poll_read(cx, buf))?;
        if buf.filled().len() == before && buf.remaining() > 0 && me.peer_reset() {
            return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()));
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Pipe {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        if me.peer_reset() {
            return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()));
        }
        Pin::new(&mut me.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        if me.peer_reset() {
            return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()));
        }
        Pin::new(&mut me.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
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
    async fn a_prefix_is_read_before_the_stream_even_one_byte_at_a_time() {
        let (a, mut b) = pipe();
        b.write_all(b" world").await.unwrap();
        drop(b);
        let mut p = Prefixed::new(b"hello".to_vec(), a);
        let mut got = Vec::new();
        let mut one = [0u8; 1];
        while p.read(&mut one).await.unwrap() == 1 {
            got.push(one[0]);
        }
        assert_eq!(got, b"hello world");
    }

    #[tokio::test]
    async fn vectored_writes_pass_through_a_prefixed_pipe_so_hyper_queues_instead_of_copying() {
        let (a, mut b) = pipe();
        assert!(a.is_write_vectored());
        let mut p = Prefixed::new(Vec::new(), a);
        assert!(p.is_write_vectored());
        let bufs = [io::IoSlice::new(b"head "), io::IoSlice::new(b"body")];
        let n = p.write_vectored(&bufs).await.unwrap();
        assert_eq!(n, 9);
        let mut got = [0u8; 9];
        b.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"head body");
    }

    #[tokio::test]
    async fn a_vectored_write_to_a_reset_peer_fails_like_a_plain_one() {
        let (a, b) = pipe();
        b.set_reset(true);
        drop(b);
        let mut a = a;
        let bufs = [io::IoSlice::new(b"x")];
        let e = a.write_vectored(&bufs).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
    }

    #[tokio::test]
    async fn a_pipe_end_closed_normally_reads_as_the_end() {
        let (mut a, mut b) = pipe();
        a.write_all(b"x").await.unwrap();
        a.shutdown().await.unwrap();
        drop(a);
        let mut got = Vec::new();
        b.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"x");
    }

    #[tokio::test]
    async fn a_pipe_end_reset_reads_as_a_reset_after_its_data() {
        let (mut a, mut b) = pipe();
        a.write_all(b"partial").await.unwrap();
        a.reset_on_close();
        drop(a);
        let mut buf = [0u8; 16];
        let n = b.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"partial");
        let e = b.read(&mut buf).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(
            b.write(b"late").await.unwrap_err().kind(),
            io::ErrorKind::ConnectionReset
        );
    }

    #[tokio::test]
    async fn a_reset_flagged_end_that_only_shuts_down_is_a_clean_half_close() {
        let (mut a, mut b) = pipe();
        a.set_reset(true);
        a.write_all(b"answer").await.unwrap();
        a.shutdown().await.unwrap();
        let mut got = Vec::new();
        b.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"answer");
        b.write_all(b"still heard").await.unwrap();
        let mut heard = [0u8; 11];
        a.read_exact(&mut heard).await.unwrap();
        assert_eq!(&heard, b"still heard");
        drop(a);
        assert_eq!(
            b.write(b"late").await.unwrap_err().kind(),
            io::ErrorKind::ConnectionReset,
            "dropping it later is a reset"
        );
    }

    #[test]
    fn the_reason_one_end_gives_for_refusing_is_read_on_the_other() {
        let (a, b) = pipe();
        let ending = Io::ending(&a).unwrap();
        assert_eq!(ending.refused(), None);
        for why in crate::Refused::ALL {
            b.refuse(why);
            assert_eq!(ending.refused(), Some(why));
        }
    }

    #[test]
    fn the_way_that_answered_is_read_on_the_other_end() {
        let (a, b) = pipe();
        let ending = Io::ending(&a).unwrap();
        assert_eq!(ending.via(), None);
        Io::ending(&b).unwrap().answered_by(42);
        assert_eq!(ending.via(), Some(42));
    }

    #[tokio::test]
    async fn waiting_for_the_end_returns_once_the_other_side_ended_even_if_it_ended_first() {
        let (a, b) = pipe();
        let ending = Io::ending(&a).unwrap();
        let waiting = tokio::spawn({
            let ending = ending.clone();
            async move { ending.ended().await }
        });
        tokio::task::yield_now().await;
        Io::ending(&b).unwrap().end();
        waiting.await.unwrap();
        ending.ended().await;
    }

    #[tokio::test]
    async fn a_reset_flag_taken_back_means_a_normal_end() {
        let (a, mut b) = pipe();
        a.set_reset(true);
        a.set_reset(false);
        drop(a);
        let mut buf = [0u8; 4];
        assert_eq!(b.read(&mut buf).await.unwrap(), 0);
    }
}
