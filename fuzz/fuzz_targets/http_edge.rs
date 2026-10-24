#![no_main]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use burrow_web::http::host::host_header;
use burrow_web::tls::{Applying, CertName, CertState, CertStatus, Certificates};
use burrow_web::{
    BoxIo, CertSettings, Listeners, Proxy, ProxySettings, Refused, Site, SiteTls, Timeouts,
    TunnelConn, TunnelMode, TunnelSettings, Upstream, Visitor,
};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response};
use libfuzzer_sys::fuzz_target;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio_rustls::rustls::server::{ClientHello, ResolvesServerCert};
use tokio_rustls::rustls::sign::CertifiedKey;

const TOKEN: &str = "tok_EN-1";
const ANSWER: &str = "tok_EN-1.thumbprint";
const APP_OK: &str = "app-ok";

const SITES: [(&str, &str, SiteTls, bool); 8] = [
    ("plain", "plain.example", SiteTls::Plain, false),
    ("nas", "nas", SiteTls::Plain, false),
    ("wild", "*.w.example", SiteTls::Plain, false),
    (
        "both",
        "both.example",
        SiteTls::Https(burrow_web::CertSource::Auto),
        false,
    ),
    (
        "secure",
        "secure.example",
        SiteTls::Https(burrow_web::CertSource::Auto),
        true,
    ),
    (
        "wait",
        "wait.example",
        SiteTls::Https(burrow_web::CertSource::Auto),
        true,
    ),
    ("pass", "pass.example", SiteTls::Passthrough, true),
    ("busy", "busy.example", SiteTls::Plain, false),
];

const POOL: [&str; 15] = [
    "tunnel.example",
    "plain.example",
    "nas",
    "a.w.example",
    "both.example",
    "secure.example",
    "wait.example",
    "pass.example",
    "busy.example",
    "PLAIN.Example:80",
    "plain.example.",
    "a.b.w.example",
    "127.0.0.1",
    "[::1]:80",
    "user@plain.example",
];

fn service_for(host: &str) -> Option<&'static str> {
    if let Some(&(service, ..)) = SITES.iter().find(|s| s.1 == host) {
        return Some(service);
    }
    let label = host.strip_suffix(".w.example")?;
    (!label.is_empty() && !label.contains('.')).then_some("wild")
}

#[derive(Debug)]
struct Certs;

impl ResolvesServerCert for Certs {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        None
    }
}

impl Certificates for Certs {
    fn state(&self, name: &str) -> CertState {
        if name == "wait.example" {
            CertState::Pending
        } else {
            CertState::Ready
        }
    }

    fn certificate(&self, _: &str) -> Option<Arc<CertifiedKey>> {
        None
    }

    fn alpn_challenge(&self, _: &str) -> Option<Arc<CertifiedKey>> {
        None
    }

    fn http_challenge(&self, token: &str) -> Option<String> {
        (token == TOKEN).then(|| ANSWER.to_string())
    }

    fn apply<'a>(&'a self, _: &'a CertSettings, _: &'a [CertName]) -> Applying<'a> {
        Box::pin(async {})
    }

    fn status(&self) -> Vec<CertStatus> {
        Vec::new()
    }
}

fn one<'a>(req: &'a Request<Incoming>, name: &str) -> Option<&'a str> {
    let mut all = req.headers().get_all(name).iter();
    let first = all.next()?;
    assert!(all.next().is_none(), "the app got {name} twice");
    Some(first.to_str().expect("a header burrow set is text"))
}

fn check_at_app(req: &Request<Incoming>) {
    let host = one(req, "host").expect("the app always gets a Host");
    let forwarded = one(req, "x-forwarded-host").expect("x-forwarded-host is set");
    assert_eq!(host_header(host).as_deref(), Some(forwarded));
    assert!(service_for(forwarded).is_some(), "{forwarded} has no site");
    assert_eq!(one(req, "x-forwarded-for"), Some("127.0.0.1"));
    assert_eq!(one(req, "x-real-ip"), Some("127.0.0.1"));
    assert_eq!(one(req, "x-forwarded-proto"), Some("http"));
    for gone in ["keep-alive", "proxy-connection", "te", "trailer"] {
        assert!(!req.headers().contains_key(gone), "{gone} reached the app");
    }
    if let Some(te) = one(req, "transfer-encoding") {
        assert_eq!(te, "chunked", "only burrow's own framing reaches the app");
    }
    let upgrade = one(req, "connection").map(str::to_ascii_lowercase);
    assert!(
        upgrade.is_none() || upgrade.as_deref() == Some("upgrade"),
        "connection: {upgrade:?}"
    );
    assert_eq!(upgrade.is_some(), req.headers().contains_key("upgrade"));
    assert!(req.uri().authority().is_none(), "the app gets origin-form");
}

async fn answer(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    check_at_app(&req);
    let _ = req.into_body().collect().await;
    Ok(Response::new(Full::new(Bytes::from_static(
        APP_OK.as_bytes(),
    ))))
}

struct App;

impl Upstream for App {
    type Permit = ();

    async fn open(&self, service: &str, from: &Visitor) -> Result<BoxIo, Refused> {
        assert_eq!(
            service_for(&from.host),
            Some(service),
            "{} was routed to {service}",
            from.host
        );
        if service == "busy" {
            return Err(Refused::Full);
        }
        let (ours, theirs) = burrow_web::pipe();
        tokio::spawn(async move {
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    hyper_util::rt::TokioIo::new(theirs),
                    hyper::service::service_fn(answer),
                )
                .await;
        });
        Ok(Box::new(ours))
    }

    fn tunnel_permit(&self, _: SocketAddr) -> Option<()> {
        Some(())
    }

    async fn tunnel(&self, conn: TunnelConn<()>) {
        assert!(conn.upgraded, "port 80 hands over only an upgraded stream");
        assert_eq!(conn.path, "/burrow");
    }
}

fn settings(http: SocketAddr) -> ProxySettings {
    let mut s = ProxySettings::new("127.0.0.1:443".parse().expect("an address"));
    s.http = Some(http);
    s.sites = SITES
        .iter()
        .map(|(service, host, tls, redirect)| Site {
            service: service.to_string(),
            hosts: vec![host.to_string()],
            tls: tls.clone(),
            https_redirect: *redirect,
        })
        .collect();
    s.tunnel = Some(TunnelSettings {
        host: "tunnel.example".into(),
        path: "/burrow".into(),
        mode: TunnelMode::OwnName,
        port: burrow_web::TunnelPort::Http,
    });
    s.timeouts = Timeouts {
        request_header: Duration::from_secs(1),
        idle: Duration::from_secs(1),
        response_header: Duration::from_secs(2),
        ..Timeouts::default()
    };
    s
}

struct World {
    rt: Runtime,
    addr: SocketAddr,
    _proxy: Proxy<App>,
}

fn world() -> &'static World {
    static WORLD: OnceLock<World> = OnceLock::new();
    WORLD.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let (addr, proxy) = rt.block_on(async {
            let http = TcpListener::bind("127.0.0.1:0").await.expect("a port");
            let addr = http.local_addr().expect("an address");
            let listeners = Listeners {
                https: None,
                http: Some(http),
            };
            let proxy = Proxy::start(listeners, settings(addr), Arc::new(App), Arc::new(Certs));
            (addr, proxy.await)
        });
        World {
            rt,
            addr,
            _proxy: proxy,
        }
    })
}

fn check_answer(sent: &[u8], got: &[u8]) {
    if got.is_empty() {
        return;
    }
    let text = String::from_utf8_lossy(got).to_ascii_lowercase();
    assert!(
        text.starts_with("http/1.1 ") || text.starts_with("http/1.0 "),
        "not an HTTP answer: {text:?}"
    );
    let code: u16 = text[9..12].parse().expect("a status code");
    assert!(
        [
            100, 101, 200, 308, 400, 403, 404, 413, 426, 431, 501, 502, 503, 505
        ]
        .contains(&code),
        "status {code} for {:?}",
        String::from_utf8_lossy(sent)
    );
    if code == 200 && !sent.starts_with(b"HEAD ") {
        assert!(
            text.contains(APP_OK) || text.contains(&ANSWER.to_ascii_lowercase()),
            "{text:?} for {:?}",
            String::from_utf8_lossy(sent)
        );
    }
    if code == 308 {
        assert!(text.contains("\r\nlocation: https://"), "{text}");
    }
}

fn exchange(sent: &[u8]) {
    let w = world();
    let got = w.rt.block_on(async {
        let mut s = TcpStream::connect(w.addr).await.expect("the proxy listens");
        let _ = s.write_all(sent).await;
        let _ = s.shutdown().await;
        let mut got = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut got)).await;
        assert!(read.is_ok(), "the connection outlived its client by 5 s");
        got
    });
    check_answer(sent, &got);
}

struct Pick<'a>(&'a [u8]);

impl<'a> Pick<'a> {
    fn byte(&mut self) -> u8 {
        let Some((&b, rest)) = self.0.split_first() else {
            return 0;
        };
        self.0 = rest;
        b
    }

    fn of<'b>(&mut self, all: &[&'b str]) -> &'b str {
        all[usize::from(self.byte()) % all.len()]
    }

    fn raw(&mut self) -> &'a [u8] {
        let n = usize::from(self.byte() % 24).min(self.0.len());
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        head
    }
}

fn target(p: &mut Pick<'_>, host: &str) -> Vec<u8> {
    let path = p.of(&[
        "/",
        "/x?y=1",
        "/burrow",
        "/.well-known/acme-challenge/",
        "*",
        "abs-http",
        "abs-https",
        "authority",
        "raw",
    ]);
    match path {
        "abs-http" => format!("http://{host}/a").into_bytes(),
        "abs-https" => format!("https://{host}/b?c").into_bytes(),
        "authority" => format!("{host}:443").into_bytes(),
        "raw" => [b"/".as_slice(), p.raw()].concat(),
        "/.well-known/acme-challenge/" if p.byte().is_multiple_of(2) => {
            format!("{path}{TOKEN}").into_bytes()
        }
        _ => path.as_bytes().to_vec(),
    }
}

fn header(p: &mut Pick<'_>, out: &mut Vec<u8>, eol: &[u8]) {
    let line = p.of(&[
        "Content-Length: 4",
        "Content-Length: 0",
        "Content-Length: 4, 5",
        "Transfer-Encoding: chunked",
        "Transfer-Encoding: xchunked",
        "Transfer-Encoding: chunked, identity",
        "Transfer-Encoding: gzip, chunked",
        "Connection: close",
        "Connection: keep-alive, X-Secret",
        "Connection: Upgrade",
        "Upgrade: websocket",
        "Sec-WebSocket-Version: 13",
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
        "X-Secret: 1",
        "X-Forwarded-For: 6.6.6.6",
        "X-Real-IP: 6.6.6.6",
        "X-Forwarded-Proto: https",
        "TE: trailers",
        "Keep-Alive: timeout=5",
        "Proxy-Connection: keep-alive",
        "Trailer: X-T",
        "Expect: 100-continue",
        " folded",
        "raw",
    ]);
    if line == "raw" {
        out.extend_from_slice(p.raw());
    } else {
        out.extend_from_slice(line.as_bytes());
    }
    out.extend_from_slice(eol);
}

fn request(p: &mut Pick<'_>, out: &mut Vec<u8>) {
    let eol: &[u8] = if p.byte().is_multiple_of(16) {
        b"\n"
    } else {
        b"\r\n"
    };
    let method = p.of(&[
        "GET", "POST", "HEAD", "PUT", "OPTIONS", "CONNECT", "DELETE", "G T",
    ]);
    let host = p.of(&POOL);
    out.extend_from_slice(method.as_bytes());
    out.push(b' ');
    out.extend(target(p, host));
    out.push(b' ');
    out.extend_from_slice(
        p.of(&["HTTP/1.1", "HTTP/1.0", "HTTP/2.0", "HTTP/1.1 "])
            .as_bytes(),
    );
    out.extend_from_slice(eol);
    for _ in 0..p.byte() % 3 {
        out.extend_from_slice(b"Host: ");
        out.extend_from_slice(if p.byte().is_multiple_of(4) {
            p.raw()
        } else {
            p.of(&POOL).as_bytes()
        });
        out.extend_from_slice(eol);
    }
    for _ in 0..p.byte() % 7 {
        header(p, out, eol);
    }
    out.extend_from_slice(eol);
    match p.byte() % 4 {
        0 => {}
        1 => out.extend_from_slice(b"abcd"),
        2 => out.extend_from_slice(b"4\r\nabcd\r\n0\r\n\r\n"),
        _ => out.extend_from_slice(p.raw()),
    }
}

fn structured(data: &[u8]) -> Vec<u8> {
    let mut p = Pick(data);
    let mut out = Vec::new();
    for _ in 0..1 + p.byte() % 3 {
        request(&mut p, &mut out);
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    if mode % 2 == 0 {
        exchange(rest);
    } else {
        exchange(&structured(rest));
    }
});
