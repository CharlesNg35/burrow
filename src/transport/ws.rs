use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{Buf, Bytes, BytesMut};
use futures_util::{Sink, Stream};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_websockets::{ClientBuilder, Limits, Message, ServerBuilder, WebSocketStream};

use super::endpoint::{Endpoint, join_host_port};

pub const MAX_REQUEST_HEAD: usize = 8 * 1024;
const MAX_RESPONSE_HEAD: usize = 64 * 1024;
const MAX_HEADERS: usize = 32;
const WRITE_BATCH: usize = 128 * 1024;
const MAX_MESSAGE: usize = WRITE_BATCH;

pub enum Carrier {
    Plain(TcpStream),
    ClientTls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
    ServerTls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

delegate_read!(Carrier, Plain, ClientTls, ServerTls);
delegate_write!(Carrier, Plain, ClientTls, ServerTls);

pub struct Rewind {
    prefix: Bytes,
    inner: Carrier,
    written: u64,
    read_cap: Option<usize>,
}

impl AsyncRead for Rewind {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        if !me.prefix.is_empty() {
            let n = me.prefix.len().min(buf.remaining());
            buf.put_slice(&me.prefix[..n]);
            me.prefix.advance(n);
            return Poll::Ready(Ok(()));
        }
        let Some(left) = me.read_cap else {
            return Pin::new(&mut me.inner).poll_read(cx, buf);
        };
        if left == 0 {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the WebSocket upgrade response is too large",
            )));
        }
        let before = buf.filled().len();
        ready!(Pin::new(&mut me.inner).poll_read(cx, buf))?;
        let n = buf.filled().len() - before;
        me.read_cap = Some(left.saturating_sub(n));
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Rewind {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        let written = Pin::new(&mut me.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = written {
            me.written = me.written.wrapping_add(n as u64);
        }
        written
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

pub struct WsConn {
    inner: WebSocketStream<Rewind>,
    read_buf: Bytes,
    write_buf: Vec<u8>,
    eof: bool,
    controls: u32,
    window: std::time::Instant,
    stalled: u32,
    written_at_ping: u64,
    keepalive: Option<Keepalive>,
}

struct Keepalive {
    every: std::time::Duration,
    active: std::time::Instant,
    timer: Pin<Box<tokio::time::Sleep>>,
    due: bool,
}

const MAX_CONTROLS: u32 = 100;
const CONTROL_WINDOW: std::time::Duration = std::time::Duration::from_secs(10);

fn ws_err(e: tokio_websockets::Error) -> io::Error {
    match e {
        tokio_websockets::Error::Io(e) => e,
        other => io::Error::new(io::ErrorKind::InvalidData, other.to_string()),
    }
}

impl WsConn {
    fn new(mut inner: WebSocketStream<Rewind>) -> Self {
        inner.get_mut().read_cap = None;
        WsConn {
            inner,
            read_buf: Bytes::new(),
            write_buf: Vec::new(),
            eof: false,
            controls: 0,
            window: std::time::Instant::now(),
            stalled: 0,
            written_at_ping: 0,
            keepalive: None,
        }
    }

    pub fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        match &self.inner.get_ref().inner {
            Carrier::Plain(s) => s.local_addr(),
            Carrier::ClientTls(s) => s.get_ref().0.local_addr(),
            Carrier::ServerTls(s) => s.get_ref().0.local_addr(),
        }
    }

    pub fn keep_alive(&mut self, every: std::time::Duration) {
        self.keepalive = Some(Keepalive {
            every,
            active: std::time::Instant::now(),
            timer: Box::pin(tokio::time::sleep(every)),
            due: false,
        });
    }

    fn active(&mut self) {
        if let Some(k) = &mut self.keepalive {
            k.active = std::time::Instant::now();
        }
    }

    fn poll_keepalive(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        let Some(k) = &mut self.keepalive else {
            return Ok(());
        };
        while k.timer.as_mut().poll(cx).is_ready() {
            if k.active.elapsed() >= k.every {
                k.due = true;
                k.active = std::time::Instant::now();
            }
            let next = tokio::time::Instant::from_std(k.active + k.every);
            k.timer.as_mut().reset(next);
        }
        if !k.due {
            return Ok(());
        }
        match Pin::new(&mut self.inner).poll_ready(cx) {
            Poll::Ready(Ok(())) => {
                k.due = false;
                Pin::new(&mut self.inner)
                    .start_send(Message::ping(Vec::new()))
                    .map_err(ws_err)?;
                let _ = Pin::new(&mut self.inner).poll_flush(cx).map_err(ws_err)?;
                Ok(())
            }
            Poll::Ready(Err(e)) => Err(ws_err(e)),
            Poll::Pending => Ok(()),
        }
    }

    pub async fn finish(&mut self) -> io::Result<()> {
        self.flush().await?;
        self.inner.get_mut().shutdown().await
    }

    fn poll_send_buffered(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.write_buf.is_empty() {
            return Poll::Ready(Ok(()));
        }
        ready!(Pin::new(&mut self.inner).poll_ready(cx)).map_err(ws_err)?;
        let payload = Bytes::from(std::mem::take(&mut self.write_buf));
        Pin::new(&mut self.inner)
            .start_send(Message::binary(payload))
            .map_err(ws_err)?;
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for WsConn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        me.poll_keepalive(cx)?;
        loop {
            if !me.read_buf.is_empty() {
                let n = me.read_buf.len().min(buf.remaining());
                buf.put_slice(&me.read_buf[..n]);
                me.read_buf.advance(n);
                return Poll::Ready(Ok(()));
            }
            if me.eof {
                return Poll::Ready(Ok(()));
            }
            match ready!(Pin::new(&mut me.inner).poll_next(cx)) {
                Some(Ok(msg)) if msg.is_binary() => {
                    me.active();
                    me.read_buf = Bytes::from(msg.into_payload())
                }
                Some(Ok(msg)) if msg.is_close() => me.eof = true,
                Some(Ok(msg)) => {
                    if msg.is_ping() {
                        let written = me.inner.get_ref().written;
                        me.stalled = if written == me.written_at_ping {
                            me.stalled + 1
                        } else {
                            0
                        };
                        me.written_at_ping = written;
                    }
                    if me.stalled > MAX_CONTROLS {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "WebSocket control frames keep arriving while the peer reads nothing",
                        )));
                    }
                    if me.window.elapsed() > CONTROL_WINDOW {
                        me.window = std::time::Instant::now();
                        me.controls = 0;
                    }
                    me.controls += 1;
                    if me.controls > MAX_CONTROLS {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "too many WebSocket control frames",
                        )));
                    }
                }
                Some(Err(e)) => return Poll::Ready(Err(ws_err(e))),
                None => me.eof = true,
            }
        }
    }
}

impl AsyncWrite for WsConn {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        if me.write_buf.len() >= WRITE_BATCH {
            ready!(me.poll_send_buffered(cx))?;
        }
        let n = buf.len().min(WRITE_BATCH - me.write_buf.len());
        me.write_buf.extend_from_slice(&buf[..n]);
        me.active();
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        ready!(me.poll_send_buffered(cx))?;
        Pin::new(&mut me.inner).poll_flush(cx).map_err(ws_err)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        ready!(me.poll_send_buffered(cx))?;
        Pin::new(&mut me.inner).poll_close(cx).map_err(ws_err)
    }
}

fn limits() -> Limits {
    Limits::default().max_payload_len(Some(MAX_MESSAGE))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    NotFound,
    UpgradeRequired,
    BadRequest,
    HeadTooLarge,
}

impl Refused {
    fn response(self) -> &'static [u8] {
        match self {
            Refused::NotFound => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            Refused::UpgradeRequired => {
                b"HTTP/1.1 426 Upgrade Required\r\nUpgrade: websocket\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            }
            Refused::BadRequest => b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            Refused::HeadTooLarge => {
                b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            }
        }
    }
}

#[derive(Debug)]
pub enum AcceptError {
    Io(io::Error),
    Refused(Refused),
    Upgrade(String),
}

impl From<io::Error> for AcceptError {
    fn from(e: io::Error) -> Self {
        AcceptError::Io(e)
    }
}

pub fn route(head: &[u8], path: &str) -> Result<(), Refused> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut headers);
    match req.parse(head) {
        Ok(httparse::Status::Complete(_)) => {}
        _ => return Err(Refused::BadRequest),
    }
    let target = req.path.unwrap_or("");
    let target_path = target.split('?').next().unwrap_or("");
    if target_path != path {
        return Err(Refused::NotFound);
    }
    let upgrade = req.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("upgrade")
            && std::str::from_utf8(h.value)
                .is_ok_and(|v| v.trim().eq_ignore_ascii_case("websocket"))
    });
    if req.method != Some("GET") || !upgrade {
        return Err(Refused::UpgradeRequired);
    }
    Ok(())
}

pub async fn accept(mut carrier: Carrier, path: &str) -> Result<WsConn, AcceptError> {
    let mut head = BytesMut::with_capacity(1024);
    loop {
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if head.len() >= MAX_REQUEST_HEAD {
            refuse(&mut carrier, Refused::HeadTooLarge).await;
            return Err(AcceptError::Refused(Refused::HeadTooLarge));
        }
        let mut chunk = [0u8; 1024];
        let want = chunk.len().min(MAX_REQUEST_HEAD - head.len());
        let n = carrier.read(&mut chunk[..want]).await?;
        if n == 0 {
            return Err(AcceptError::Io(io::ErrorKind::UnexpectedEof.into()));
        }
        head.extend_from_slice(&chunk[..n]);
    }
    if let Err(r) = route(&head, path) {
        refuse(&mut carrier, r).await;
        return Err(AcceptError::Refused(r));
    }
    let stream = Rewind {
        prefix: head.freeze(),
        inner: carrier,
        written: 0,
        read_cap: None,
    };
    let (_, ws) = ServerBuilder::new()
        .limits(limits())
        .accept(stream)
        .await
        .map_err(|e| AcceptError::Upgrade(e.to_string()))?;
    Ok(WsConn::new(ws))
}

async fn refuse(carrier: &mut Carrier, r: Refused) {
    let _ = carrier.write_all(r.response()).await;
    let _ = carrier.shutdown().await;
}

#[derive(Debug)]
pub enum ConnectError {
    Io(io::Error),
    Status(u16),
    Handshake(String),
}

impl ConnectError {
    pub fn describe(&self) -> String {
        match self {
            ConnectError::Io(e) => crate::failure::io_reason(e),
            ConnectError::Status(code) => {
                format!("the server answered HTTP {code} instead of upgrading to WebSocket")
            }
            ConnectError::Handshake(e) => format!("WebSocket handshake failed ({e})"),
        }
    }
}

pub async fn connect(
    carrier: Carrier,
    endpoint: &Endpoint,
    headers: &[(String, String)],
) -> Result<WsConn, ConnectError> {
    let Endpoint::WebSocket {
        tls,
        host,
        port,
        path,
    } = endpoint
    else {
        return Err(ConnectError::Handshake("not a WebSocket endpoint".into()));
    };
    let scheme = if *tls { "wss" } else { "ws" };
    let uri = format!("{scheme}://{}{path}", join_host_port(host, *port));
    let mut builder = ClientBuilder::new()
        .uri(&uri)
        .map_err(|e| ConnectError::Handshake(e.to_string()))?
        .limits(limits());
    for (name, value) in headers {
        let name = http::HeaderName::try_from(name.as_str())
            .map_err(|e| ConnectError::Handshake(e.to_string()))?;
        let value = http::HeaderValue::try_from(value.as_str())
            .map_err(|e| ConnectError::Handshake(e.to_string()))?;
        builder = builder
            .add_header(name, value)
            .map_err(|e| ConnectError::Handshake(e.to_string()))?;
    }
    let stream = Rewind {
        prefix: Bytes::new(),
        inner: carrier,
        written: 0,
        read_cap: Some(MAX_RESPONSE_HEAD),
    };
    match builder.connect_on(stream).await {
        Ok((ws, _)) => Ok(WsConn::new(ws)),
        Err(tokio_websockets::Error::Upgrade(
            tokio_websockets::upgrade::Error::DidNotSwitchProtocols(code),
        )) => Err(ConnectError::Status(code)),
        Err(tokio_websockets::Error::Io(e)) => Err(ConnectError::Io(e)),
        Err(e) => Err(ConnectError::Handshake(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn send_message(len: usize) -> io::Result<usize> {
        use futures_util::SinkExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut conn = accept(Carrier::Plain(tcp), "/burrow").await.unwrap();
            let mut got = vec![0u8; len];
            conn.read_exact(&mut got).await
        });
        let tcp = TcpStream::connect(addr).await.unwrap();
        let (mut ws, _) = ClientBuilder::new()
            .uri(&format!("ws://{addr}/burrow"))
            .unwrap()
            .connect_on(tcp)
            .await
            .unwrap();
        let _ = ws.send(Message::binary(vec![7u8; len])).await;
        server.await.unwrap()
    }

    #[tokio::test]
    async fn messages_larger_than_a_write_batch_are_refused() {
        assert_eq!(send_message(WRITE_BATCH).await.unwrap(), WRITE_BATCH);
        assert!(send_message(WRITE_BATCH + 1).await.is_err());
    }

    async fn after_pings(pings: usize) -> io::Result<usize> {
        use futures_util::SinkExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut conn = accept(Carrier::Plain(tcp), "/burrow").await.unwrap();
            let mut got = [0u8; 4];
            conn.read_exact(&mut got).await
        });
        let tcp = TcpStream::connect(addr).await.unwrap();
        let (mut ws, _) = ClientBuilder::new()
            .uri(&format!("ws://{addr}/burrow"))
            .unwrap()
            .connect_on(tcp)
            .await
            .unwrap();
        for _ in 0..pings {
            if ws.send(Message::ping(Vec::new())).await.is_err() {
                break;
            }
        }
        let _ = ws.send(Message::binary(vec![7u8; 4])).await;
        server.await.unwrap()
    }

    async fn pinging_while_reading(read_every: Option<std::time::Duration>) -> bool {
        use futures_util::{SinkExt, StreamExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let conn = accept(Carrier::Plain(tcp), "/burrow").await.unwrap();
            let (mut rd, mut wr) = tokio::io::split(conn);
            let writer = tokio::spawn(async move {
                let chunk = vec![1u8; 64 * 1024];
                while wr.write_all(&chunk).await.is_ok() && wr.flush().await.is_ok() {}
            });
            let mut buf = [0u8; 64];
            let ended = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                loop {
                    match rd.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                }
            })
            .await
            .is_ok();
            writer.abort();
            ended
        });
        let tcp = TcpStream::connect(addr).await.unwrap();
        let (ws, _) = ClientBuilder::new()
            .uri(&format!("ws://{addr}/burrow"))
            .unwrap()
            .connect_on(tcp)
            .await
            .unwrap();
        let mut ws = ws;
        for _ in 0..(MAX_CONTROLS as usize + 30) {
            if ws.send(Message::ping(vec![0u8; 125])).await.is_err() {
                break;
            }
            let pause = std::time::Duration::from_millis(105);
            match read_every {
                None => tokio::time::sleep(pause).await,
                Some(every) => {
                    let _ = tokio::time::timeout(pause, async {
                        while let Some(Ok(_)) = ws.next().await {
                            tokio::time::sleep(every).await;
                        }
                    })
                    .await;
                }
            }
        }
        if read_every.is_some() {
            let cut = server.is_finished();
            server.abort();
            return cut;
        }
        server.await.unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pings_from_a_peer_that_reads_nothing_end_the_connection() {
        assert!(pinging_while_reading(None).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pings_from_a_slow_reader_do_not_end_the_connection() {
        let slow = Some(std::time::Duration::from_millis(20));
        assert!(!pinging_while_reading(slow).await);
    }

    #[tokio::test]
    async fn a_quiet_server_connection_pings_its_peer() {
        use futures_util::StreamExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut conn = accept(Carrier::Plain(tcp), "/burrow").await.unwrap();
            conn.keep_alive(std::time::Duration::from_millis(200));
            let mut buf = [0u8; 16];
            let _ = conn.read(&mut buf).await;
        });
        let tcp = TcpStream::connect(addr).await.unwrap();
        let (mut ws, _) = ClientBuilder::new()
            .uri(&format!("ws://{addr}/burrow"))
            .unwrap()
            .connect_on(tcp)
            .await
            .unwrap();
        let mut pings = 0;
        let quiet = tokio::time::timeout(std::time::Duration::from_millis(900), async {
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_ping() {
                    pings += 1;
                }
            }
        })
        .await;
        assert!(quiet.is_err());
        assert!((2..=5).contains(&pings), "{pings} pings");
        server.abort();
    }

    #[tokio::test]
    async fn an_endless_upgrade_response_is_refused() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let sent = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut req = [0u8; 4096];
            let _ = tcp.read(&mut req).await;
            let _ = tcp
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\nX: ")
                .await;
            let filler = vec![b'a'; 64 * 1024];
            let mut total = 0usize;
            while total < 64 << 20 && tcp.write_all(&filler).await.is_ok() {
                total += filler.len();
            }
            total
        });
        let tcp = TcpStream::connect(addr).await.unwrap();
        let endpoint = Endpoint::parse(&format!("ws://{addr}/burrow")).unwrap();
        let got = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            connect(Carrier::Plain(tcp), &endpoint, &[]),
        )
        .await
        .expect("the upgrade gives up");
        assert!(got.is_err());
        assert!(sent.await.unwrap() < 16 << 20);
    }

    #[tokio::test]
    async fn a_flood_of_control_frames_ends_the_connection() {
        assert_eq!(after_pings(MAX_CONTROLS as usize).await.unwrap(), 4);
        assert!(after_pings(MAX_CONTROLS as usize + 50).await.is_err());
    }

    fn req(path: &str, extra: &str) -> Vec<u8> {
        format!("GET {path} HTTP/1.1\r\nHost: x\r\n{extra}\r\n").into_bytes()
    }

    #[test]
    fn routes_only_the_configured_upgrade() {
        let up = "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: a\r\nSec-WebSocket-Version: 13\r\n";
        assert_eq!(route(&req("/burrow", up), "/burrow"), Ok(()));
        assert_eq!(route(&req("/burrow?x=1", up), "/burrow"), Ok(()));
        assert_eq!(route(&req("/other", up), "/burrow"), Err(Refused::NotFound));
        assert_eq!(
            route(&req("/burrow", ""), "/burrow"),
            Err(Refused::UpgradeRequired)
        );
        assert_eq!(
            route(
                b"POST /burrow HTTP/1.1\r\nUpgrade: websocket\r\n\r\n",
                "/burrow"
            ),
            Err(Refused::UpgradeRequired)
        );
        assert_eq!(
            route(b"garbage\r\n\r\n", "/burrow"),
            Err(Refused::BadRequest)
        );
        let many: String = (0..40).map(|i| format!("X-{i}: y\r\n")).collect();
        assert_eq!(
            route(&req("/burrow", &many), "/burrow"),
            Err(Refused::BadRequest)
        );
    }
}
