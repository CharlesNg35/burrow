//! Lingering close for HTTP/1 servers: after the last answer, read and drop what the client still sends, within a byte and a time cap, then close.
//! Closing with unread data sends a TCP reset, and a reset can make the client lose an answer it was already sent (401, 413...).
//! It must not depend on any other burrow crate.

#![forbid(unsafe_code)]

use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use hyper::body::{Body, Incoming};
use hyper::server::conn::http1::{Connection, UpgradeableConnection};
use hyper::service::HttpService;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const LINGER_TIME: Duration = Duration::from_secs(10);
pub const LINGER_TIMEOUT: Duration = Duration::from_secs(5);
pub const LINGER_BYTES: u64 = 1024 * 1024;
const CHUNK: usize = 8 * 1024;

type BoxError = Box<dyn Error + Send + Sync>;

pub trait Http1: Future<Output = hyper::Result<()>> + Unpin {
    type Io: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn graceful_shutdown(self: Pin<&mut Self>);

    fn into_io(self) -> Option<Self::Io>;
}

impl<T, S, B> Http1 for Connection<TokioIo<T>, S>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: HttpService<Incoming, ResBody = B>,
    S::Error: Into<BoxError>,
    B: Body + 'static,
    B::Error: Into<BoxError>,
    Self: Unpin,
{
    type Io = T;

    fn graceful_shutdown(self: Pin<&mut Self>) {
        Connection::graceful_shutdown(self);
    }

    fn into_io(self) -> Option<T> {
        Some(self.into_parts().io.into_inner())
    }
}

impl<T, S, B> Http1 for UpgradeableConnection<TokioIo<T>, S>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: HttpService<Incoming, ResBody = B>,
    S::Error: Into<BoxError>,
    B: Body + 'static,
    B::Error: Into<BoxError>,
    Self: Unpin,
{
    type Io = T;

    fn graceful_shutdown(self: Pin<&mut Self>) {
        UpgradeableConnection::graceful_shutdown(self);
    }

    fn into_io(self) -> Option<T> {
        self.into_parts().map(|p| p.io.into_inner())
    }
}

enum State<C> {
    Serving(C),
    Closing(Pin<Box<dyn Future<Output = ()> + Send>>),
    Done,
}

pub struct Lingering<C> {
    state: State<C>,
}

pub fn serve<C: Http1>(conn: C) -> Lingering<C> {
    Lingering {
        state: State::Serving(conn),
    }
}

impl<C: Http1> Lingering<C> {
    pub fn graceful_shutdown(self: Pin<&mut Self>) {
        if let State::Serving(conn) = &mut self.get_mut().state {
            Pin::new(conn).graceful_shutdown();
        }
    }
}

impl<C: Http1> Future for Lingering<C> {
    type Output = hyper::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = self.get_mut();
        loop {
            match &mut me.state {
                State::Serving(conn) => {
                    ready!(Pin::new(conn).poll(cx))?;
                    let State::Serving(conn) = std::mem::replace(&mut me.state, State::Done) else {
                        return Poll::Ready(Ok(()));
                    };
                    match conn.into_io() {
                        Some(io) => me.state = State::Closing(Box::pin(linger(io))),
                        None => return Poll::Ready(Ok(())),
                    }
                }
                State::Closing(closing) => {
                    ready!(closing.as_mut().poll(cx));
                    me.state = State::Done;
                    return Poll::Ready(Ok(()));
                }
                State::Done => return Poll::Ready(Ok(())),
            }
        }
    }
}

pub async fn linger<T: AsyncRead + AsyncWrite + Unpin>(io: T) {
    let mut io = io;
    let _ = tokio::time::timeout(LINGER_TIME, async {
        let _ = io.shutdown().await;
        let mut left = (&mut io).take(LINGER_BYTES);
        let mut chunk = vec![0u8; CHUNK];
        while let Ok(Ok(1..)) = tokio::time::timeout(LINGER_TIMEOUT, left.read(&mut chunk)).await {}
    })
    .await;
}

#[cfg(any(test, feature = "probe"))]
pub mod probe;

#[cfg(test)]
mod tests;
