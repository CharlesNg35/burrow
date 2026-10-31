use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::net::tcp;

use super::linger::{Linger, Split};

pub enum Incoming {
    Tcp(TcpStream),
    #[cfg(feature = "web")]
    Web(burrow_web::Pipe),
}

pub enum IncomingRead<'a> {
    Tcp(tcp::ReadHalf<'a>),
    #[cfg(feature = "web")]
    Web(tokio::io::ReadHalf<&'a mut burrow_web::Pipe>),
}

pub enum IncomingWrite<'a> {
    Tcp(tcp::WriteHalf<'a>),
    #[cfg(feature = "web")]
    Web(tokio::io::WriteHalf<&'a mut burrow_web::Pipe>),
}

impl Incoming {
    pub fn finished_cleanly(&self) {
        #[cfg(feature = "web")]
        if let Incoming::Web(p) = self {
            p.set_reset(false);
        }
    }

    #[cfg(feature = "web")]
    pub fn refused(&self, why: Option<burrow_web::Refused>) {
        if let (Incoming::Web(p), Some(why)) = (self, why) {
            p.refuse(why);
        }
    }
}

impl Split for Incoming {
    type Read<'a> = IncomingRead<'a>;
    type Write<'a> = IncomingWrite<'a>;

    fn halves(&mut self) -> (Self::Read<'_>, Self::Write<'_>) {
        match self {
            Incoming::Tcp(s) => {
                let (r, w) = s.halves();
                (IncomingRead::Tcp(r), IncomingWrite::Tcp(w))
            }
            #[cfg(feature = "web")]
            Incoming::Web(p) => {
                let (r, w) = tokio::io::split(p);
                (IncomingRead::Web(r), IncomingWrite::Web(w))
            }
        }
    }

    fn reset_on_close(&self, reset: bool) {
        match self {
            Incoming::Tcp(s) => s.reset_on_close(reset),
            #[cfg(feature = "web")]
            Incoming::Web(p) => p.set_reset(reset),
        }
    }
}

impl Linger for IncomingWrite<'_> {
    fn close_normally(&self) {
        match self {
            IncomingWrite::Tcp(w) => w.close_normally(),
            #[cfg(feature = "web")]
            IncomingWrite::Web(_) => {}
        }
    }
}

impl AsyncRead for IncomingRead<'_> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            IncomingRead::Tcp(r) => Pin::new(r).poll_read(cx, buf),
            #[cfg(feature = "web")]
            IncomingRead::Web(r) => Pin::new(r).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for IncomingWrite<'_> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            IncomingWrite::Tcp(w) => Pin::new(w).poll_write(cx, buf),
            #[cfg(feature = "web")]
            IncomingWrite::Web(w) => Pin::new(w).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            IncomingWrite::Tcp(w) => Pin::new(w).poll_flush(cx),
            #[cfg(feature = "web")]
            IncomingWrite::Web(w) => Pin::new(w).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            IncomingWrite::Tcp(w) => Pin::new(w).poll_shutdown(cx),
            #[cfg(feature = "web")]
            IncomingWrite::Web(w) => Pin::new(w).poll_shutdown(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            IncomingWrite::Tcp(w) => Pin::new(w).poll_write_vectored(cx, bufs),
            #[cfg(feature = "web")]
            IncomingWrite::Web(w) => Pin::new(w).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            IncomingWrite::Tcp(w) => w.is_write_vectored(),
            #[cfg(feature = "web")]
            IncomingWrite::Web(w) => w.is_write_vectored(),
        }
    }
}
