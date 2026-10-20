use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::net::tcp as tokio_tcp;

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

pub mod endpoint;
pub mod local;
pub mod proxy;
pub mod resolve;
pub mod tcp;
pub mod tls;
pub mod udp;
pub mod ws;

pub enum Conn {
    Tcp(TcpStream),
    Ws(Box<ws::WsConn>),
    Local(tokio::io::DuplexStream),
}

pub enum ReadHalf<'a> {
    Tcp(tokio_tcp::ReadHalf<'a>),
    Ws(tokio::io::ReadHalf<&'a mut ws::WsConn>),
    Local(tokio::io::ReadHalf<&'a mut tokio::io::DuplexStream>),
}

pub enum WriteHalf<'a> {
    Tcp(tokio_tcp::WriteHalf<'a>),
    Ws(tokio::io::WriteHalf<&'a mut ws::WsConn>),
    Local(tokio::io::WriteHalf<&'a mut tokio::io::DuplexStream>),
}

pub enum OwnedReadHalf {
    Tcp(tokio_tcp::OwnedReadHalf),
    Ws(tokio::io::ReadHalf<ws::WsConn>),
    Local(tokio::io::ReadHalf<tokio::io::DuplexStream>),
}

pub enum OwnedWriteHalf {
    Tcp(tokio_tcp::OwnedWriteHalf),
    Ws(tokio::io::WriteHalf<ws::WsConn>),
    Local(tokio::io::WriteHalf<tokio::io::DuplexStream>),
}

impl Conn {
    pub fn split(&mut self) -> (ReadHalf<'_>, WriteHalf<'_>) {
        match self {
            Conn::Tcp(s) => {
                let (r, w) = s.split();
                (ReadHalf::Tcp(r), WriteHalf::Tcp(w))
            }
            Conn::Ws(s) => {
                let (r, w) = tokio::io::split(&mut **s);
                (ReadHalf::Ws(r), WriteHalf::Ws(w))
            }
            Conn::Local(s) => {
                let (r, w) = tokio::io::split(s);
                (ReadHalf::Local(r), WriteHalf::Local(w))
            }
        }
    }

    pub fn into_split(self) -> (OwnedReadHalf, OwnedWriteHalf) {
        match self {
            Conn::Tcp(s) => {
                let (r, w) = s.into_split();
                (OwnedReadHalf::Tcp(r), OwnedWriteHalf::Tcp(w))
            }
            Conn::Ws(s) => {
                let (r, w) = tokio::io::split(*s);
                (OwnedReadHalf::Ws(r), OwnedWriteHalf::Ws(w))
            }
            Conn::Local(s) => {
                let (r, w) = tokio::io::split(s);
                (OwnedReadHalf::Local(r), OwnedWriteHalf::Local(w))
            }
        }
    }

    pub async fn finish(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Tcp(_) | Conn::Local(_) => Ok(()),
            Conn::Ws(s) => s.finish().await,
        }
    }

    pub fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        match self {
            Conn::Tcp(s) => s.local_addr(),
            Conn::Ws(s) => s.local_addr(),
            Conn::Local(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "an in-memory channel has no address",
            )),
        }
    }

    pub fn is_websocket(&self) -> bool {
        matches!(self, Conn::Ws(_))
    }

    pub fn supports_half_close(&self) -> bool {
        matches!(self, Conn::Tcp(_))
    }

    pub fn reset_on_close(&self) {
        if let Conn::Tcp(s) = self {
            let _ = socket2::SockRef::from(s).set_linger(Some(std::time::Duration::ZERO));
        }
    }

    pub fn close_normally(&self) {
        if let Conn::Tcp(s) = self {
            let _ = socket2::SockRef::from(s).set_linger(None);
        }
    }
}

impl From<TcpStream> for Conn {
    fn from(s: TcpStream) -> Self {
        Conn::Tcp(s)
    }
}

delegate_read!(Conn, Tcp, Ws, Local);
delegate_write!(Conn, Tcp, Ws, Local);
delegate_read!(ReadHalf<'_>, Tcp, Ws, Local);
delegate_write!(WriteHalf<'_>, Tcp, Ws, Local);
delegate_read!(OwnedReadHalf, Tcp, Ws, Local);
delegate_write!(OwnedWriteHalf, Tcp, Ws, Local);

impl From<ws::WsConn> for Conn {
    fn from(s: ws::WsConn) -> Self {
        Conn::Ws(Box::new(s))
    }
}
