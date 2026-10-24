use std::io;
use std::time::Duration;

use tokio::net::TcpStream;

use super::CLOSE_TIMEOUT;
use crate::wire::transport::Conn;

pub trait Linger {
    fn close_normally(&self) {}
}

impl<T: Linger + ?Sized> Linger for &mut T {
    fn close_normally(&self) {
        (**self).close_normally();
    }
}

impl Linger for crate::wire::transport::WriteHalf<'_> {
    fn close_normally(&self) {
        if let crate::wire::transport::WriteHalf::Tcp(w) = self {
            let _ = socket2::SockRef::from(w.as_ref()).set_linger(None);
        }
    }
}

impl Linger for tokio::net::tcp::WriteHalf<'_> {
    fn close_normally(&self) {
        let _ = socket2::SockRef::from(self.as_ref()).set_linger(None);
    }
}

#[cfg(unix)]
impl Linger for tokio::net::unix::WriteHalf<'_> {}

pub(super) struct Graceful<W> {
    inner: W,
    restored: bool,
}

impl<W> Graceful<W> {
    pub(super) fn new(inner: W) -> Self {
        Graceful {
            inner,
            restored: false,
        }
    }
}

impl<W: tokio::io::AsyncWrite + Linger + Unpin> tokio::io::AsyncWrite for Graceful<W> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        if !self.restored {
            self.inner.close_normally();
            self.restored = true;
        }
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub trait Split {
    type Read<'a>: tokio::io::AsyncRead + Unpin + Send
    where
        Self: 'a;
    type Write<'a>: tokio::io::AsyncWrite + Linger + Unpin + Send
    where
        Self: 'a;
    fn halves(&mut self) -> (Self::Read<'_>, Self::Write<'_>);
    fn reset_on_close(&self, _reset: bool) {}
}

impl Split for TcpStream {
    type Read<'a> = tokio::net::tcp::ReadHalf<'a>;
    type Write<'a> = tokio::net::tcp::WriteHalf<'a>;
    fn halves(&mut self) -> (Self::Read<'_>, Self::Write<'_>) {
        self.split()
    }
    fn reset_on_close(&self, reset: bool) {
        let linger = reset.then_some(Duration::ZERO);
        let _ = socket2::SockRef::from(self).set_linger(linger);
    }
}

#[cfg(feature = "web")]
impl Split for burrow_web::Pipe {
    type Read<'a> = tokio::io::ReadHalf<&'a mut burrow_web::Pipe>;
    type Write<'a> = tokio::io::WriteHalf<&'a mut burrow_web::Pipe>;
    fn halves(&mut self) -> (Self::Read<'_>, Self::Write<'_>) {
        tokio::io::split(self)
    }
    fn reset_on_close(&self, reset: bool) {
        self.set_reset(reset);
    }
}

#[cfg(feature = "web")]
impl Linger for tokio::io::WriteHalf<&mut burrow_web::Pipe> {}

#[cfg(unix)]
impl Split for tokio::net::UnixStream {
    type Read<'a> = tokio::net::unix::ReadHalf<'a>;
    type Write<'a> = tokio::net::unix::WriteHalf<'a>;
    fn halves(&mut self) -> (Self::Read<'_>, Self::Write<'_>) {
        self.split()
    }
}

pub(super) async fn close(stream: &mut Conn) {
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, stream.finish()).await;
}
