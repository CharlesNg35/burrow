use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use burrow_web::{Proxy, ProxySettings};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;

use crate::common::*;

pub const HOST: &str = "site.example.com";
pub const OTHER: &str = "other.example.com";
pub const H2: &[u8] = b"h2";
pub const H1: &[u8] = b"http/1.1";

#[derive(Default)]
pub struct Counts {
    pub requests: AtomicUsize,
    pub in_flight: AtomicUsize,
    pub most_in_flight: AtomicUsize,
    pub conns: AtomicUsize,
    pub live: AtomicUsize,
    pub most_live: AtomicUsize,
    pub cut: AtomicUsize,
}

impl Counts {
    pub fn get(n: &AtomicUsize) -> usize {
        n.load(Ordering::SeqCst)
    }
}

fn up(now: &AtomicUsize, most: &AtomicUsize) {
    let n = now.fetch_add(1, Ordering::SeqCst) + 1;
    most.fetch_max(n, Ordering::SeqCst);
}

struct Down<'a>(&'a AtomicUsize);

impl Drop for Down<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct App {
    pub addr: SocketAddr,
    pub counts: Arc<Counts>,
}

type AppBody = http_body_util::combinators::BoxBody<Bytes, Infallible>;

fn streamed(counts: Arc<Counts>, total: usize, wait: Duration) -> AppBody {
    let (tx, rx) = mpsc::channel::<Result<Frame<Bytes>, Infallible>>(1);
    tokio::spawn(async move {
        tokio::time::sleep(wait).await;
        let mut left = total;
        while left > 0 {
            let n = left.min(16 * 1024);
            if tx
                .send(Ok(Frame::data(Bytes::from(vec![b'z'; n]))))
                .await
                .is_err()
            {
                counts.cut.fetch_add(1, Ordering::SeqCst);
                return;
            }
            left -= n;
        }
    });
    let stream =
        futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|f| (f, rx)) });
    StreamBody::new(stream).boxed()
}

async fn echo(req: hyper::Request<Incoming>) -> String {
    let mut text = format!("{} {} {:?}\n", req.method(), req.uri(), req.version());
    for (k, v) in req.headers() {
        text.push_str(&format!("{k}: {}\n", v.to_str().unwrap_or("?")));
    }
    let body = req
        .into_body()
        .collect()
        .await
        .map_or(0, |b| b.to_bytes().len());
    text.push_str(&format!("body-len: {body}\n"));
    text
}

async fn answer(
    counts: Arc<Counts>,
    req: hyper::Request<Incoming>,
) -> Result<hyper::Response<AppBody>, Infallible> {
    counts.requests.fetch_add(1, Ordering::SeqCst);
    up(&counts.in_flight, &counts.most_in_flight);
    let _down = Down(&counts.in_flight);
    let path = req.uri().path().to_string();
    let number = |p: &str| path.strip_prefix(p).and_then(|n| n.parse::<u64>().ok());
    if let Some(n) = number("/big/") {
        let n = usize::try_from(n).unwrap();
        return Ok(hyper::Response::new(streamed(
            counts.clone(),
            n,
            Duration::ZERO,
        )));
    }
    if let Some(ms) = number("/quiet/") {
        let wait = Duration::from_millis(ms);
        return Ok(hyper::Response::new(streamed(counts.clone(), 4, wait)));
    }
    if let Some(ms) = number("/hold/") {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    let text = echo(req).await;
    Ok(hyper::Response::new(Full::new(Bytes::from(text)).boxed()))
}

pub async fn app() -> App {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let counts = Arc::new(Counts::default());
    let c = counts.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            let c = c.clone();
            tokio::spawn(async move {
                c.conns.fetch_add(1, Ordering::SeqCst);
                up(&c.live, &c.most_live);
                let _down = Down(&c.live);
                let service = {
                    let c = c.clone();
                    hyper::service::service_fn(move |req| answer(c.clone(), req))
                };
                let _ = hyper::server::conn::http1::Builder::new()
                    .max_headers(300)
                    .serve_connection(hyper_util::rt::TokioIo::new(s), service)
                    .await;
            });
        }
    });
    App { addr, counts }
}

pub async fn proxy(app: &App, tweak: impl FnOnce(&mut ProxySettings)) -> Proxy<Fake> {
    let (up, _) = Fake::new(4);
    up.serve("site", app.addr);
    up.serve("other", app.addr);
    start_with(
        |s| {
            s.sites = vec![https_site("site", &[HOST]), https_site("other", &[OTHER])];
            tweak(s);
        },
        up,
        fixed(&[HOST, OTHER]),
        false,
    )
    .await
}

pub async fn tls_with(
    addr: SocketAddr,
    name: &str,
    alpn: &[&[u8]],
) -> std::io::Result<TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(addr).await?;
    tcp.set_nodelay(true)?;
    let name = ServerName::try_from(name.to_string()).unwrap();
    let c = connector_alpn(alpn).connect(name, tcp);
    tokio::time::timeout(Duration::from_secs(5), c)
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?
}

pub fn alpn_of(t: &TlsStream<TcpStream>) -> Option<Vec<u8>> {
    t.get_ref().1.alpn_protocol().map(<[u8]>::to_vec)
}

pub type Client = h2::client::SendRequest<Bytes>;

pub async fn client_with(addr: SocketAddr, b: &h2::client::Builder) -> Client {
    let t = tls_with(addr, HOST, &[H2, H1]).await.unwrap();
    assert_eq!(
        alpn_of(&t).as_deref(),
        Some(H2),
        "a browser offering h2 gets h2"
    );
    let (c, conn) = b.handshake::<_, Bytes>(t).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    c.ready().await.unwrap()
}

pub async fn client(addr: SocketAddr) -> Client {
    client_with(addr, &h2::client::Builder::new()).await
}

pub fn request(uri: &str, headers: &[(&str, &str)]) -> http::Request<()> {
    let mut b = http::Request::get(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    b.body(()).unwrap()
}

pub async fn read_body(mut body: h2::RecvStream) -> Result<Bytes, h2::Error> {
    let mut out = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk?;
        let _ = body.flow_control().release_capacity(chunk.len());
        out.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(out))
}

pub async fn send(c: &mut Client, req: http::Request<()>, body: Option<Bytes>) -> (u16, String) {
    let mut c = c.clone().ready().await.unwrap();
    let (resp, mut tx) = c.send_request(req, body.is_none()).unwrap();
    if let Some(b) = body {
        tx.reserve_capacity(b.len());
        tx.send_data(b, true).unwrap();
    }
    let resp = tokio::time::timeout(Duration::from_secs(10), resp)
        .await
        .expect("an answer in time")
        .unwrap();
    let status = resp.status().as_u16();
    let body = read_body(resp.into_body()).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

pub async fn get(c: &mut Client, uri: &str) -> (u16, String) {
    send(c, request(uri, &[]), None).await
}

pub fn site_uri(path: &str) -> String {
    format!("https://{HOST}{path}")
}

pub const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
pub const SETTINGS: u8 = 4;
pub const PING: u8 = 6;
pub const ACK: u8 = 0x1;

pub fn frame(ty: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let n = u32::try_from(payload.len()).unwrap();
    let mut f = n.to_be_bytes()[1..].to_vec();
    f.extend_from_slice(&[ty, flags]);
    f.extend_from_slice(&stream.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

pub struct Raw {
    pub rd: ReadHalf<TlsStream<TcpStream>>,
    pub wr: WriteHalf<TlsStream<TcpStream>>,
}

#[derive(Debug, Clone)]
pub struct Got {
    pub ty: u8,
    pub flags: u8,
    pub stream: u32,
    pub payload: Vec<u8>,
}

pub async fn raw(addr: SocketAddr) -> Raw {
    let t = tls_with(addr, HOST, &[H2]).await.unwrap();
    assert_eq!(alpn_of(&t).as_deref(), Some(H2));
    let (rd, mut wr) = tokio::io::split(t);
    let mut start = PREFACE.to_vec();
    start.extend(frame(SETTINGS, 0, 0, &[]));
    start.extend(frame(SETTINGS, ACK, 0, &[]));
    wr.write_all(&start).await.unwrap();
    Raw { rd, wr }
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, within: Duration) -> Option<Got> {
    let work = async {
        let mut h = [0u8; 9];
        r.read_exact(&mut h).await.ok()?;
        let len = usize::try_from(u32::from_be_bytes([0, h[0], h[1], h[2]])).unwrap();
        let mut payload = vec![0u8; len];
        r.read_exact(&mut payload).await.ok()?;
        Some(Got {
            ty: h[3],
            flags: h[4],
            stream: u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff,
            payload,
        })
    };
    tokio::time::timeout(within, work).await.ok().flatten()
}
