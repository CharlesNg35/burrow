pub mod proxy;
pub mod resolve;
pub mod tcp;

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::net::tcp as tokio_tcp;

pub enum Conn {
    Tcp(TcpStream),
}

pub enum ReadHalf<'a> {
    Tcp(tokio_tcp::ReadHalf<'a>),
}

pub enum WriteHalf<'a> {
    Tcp(tokio_tcp::WriteHalf<'a>),
}

pub enum OwnedReadHalf {
    Tcp(tokio_tcp::OwnedReadHalf),
}

pub enum OwnedWriteHalf {
    Tcp(tokio_tcp::OwnedWriteHalf),
}

impl Conn {
    pub fn split(&mut self) -> (ReadHalf<'_>, WriteHalf<'_>) {
        match self {
            Conn::Tcp(s) => {
                let (r, w) = s.split();
                (ReadHalf::Tcp(r), WriteHalf::Tcp(w))
            }
        }
    }

    pub fn into_split(self) -> (OwnedReadHalf, OwnedWriteHalf) {
        match self {
            Conn::Tcp(s) => {
                let (r, w) = s.into_split();
                (OwnedReadHalf::Tcp(r), OwnedWriteHalf::Tcp(w))
            }
        }
    }

    pub fn idle_and_open(&self) -> bool {
        match self {
            Conn::Tcp(s) => {
                let mut probe = [0u8; 1];
                matches!(s.try_read(&mut probe), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
            }
        }
    }

    pub fn supports_half_close(&self) -> bool {
        matches!(self, Conn::Tcp(_))
    }
}

impl From<TcpStream> for Conn {
    fn from(s: TcpStream) -> Self {
        Conn::Tcp(s)
    }
}

macro_rules! delegate_read {
    ($ty:ty, $($variant:ident),+) => {
        impl AsyncRead for $ty {
            fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
                match self.get_mut() {
                    $(Self::$variant(s) => Pin::new(s).poll_read(cx, buf),)+
                }
            }
        }
    };
}

macro_rules! delegate_write {
    ($ty:ty, $($variant:ident),+) => {
        impl AsyncWrite for $ty {
            fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
                match self.get_mut() {
                    $(Self::$variant(s) => Pin::new(s).poll_write(cx, buf),)+
                }
            }

            fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                match self.get_mut() {
                    $(Self::$variant(s) => Pin::new(s).poll_flush(cx),)+
                }
            }

            fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                match self.get_mut() {
                    $(Self::$variant(s) => Pin::new(s).poll_shutdown(cx),)+
                }
            }
        }
    };
}

delegate_read!(Conn, Tcp);
delegate_write!(Conn, Tcp);
delegate_read!(ReadHalf<'_>, Tcp);
delegate_write!(WriteHalf<'_>, Tcp);
delegate_read!(OwnedReadHalf, Tcp);
delegate_write!(OwnedWriteHalf, Tcp);
