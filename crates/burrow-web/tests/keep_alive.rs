mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use burrow_web::auth::basic::encode;
use burrow_web::{BasicUser, Refused, Site};
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const HOST: &str = "blog.example.com";
const SMALL: usize = 4096;
const OVER: usize = 300 * 1024;

struct Setup {
    proxy: burrow_web::Proxy<Fake>,
    up: Arc<Fake>,
    app: HttpBackend,
}

fn protected() -> Site {
    let mut s = plain_site("blog", &[HOST]);
    s.options.basic_auth = vec![BasicUser {
        user: "bob".into(),
        hash: bcrypt::hash("pw", 4).unwrap(),
    }];
    s
}

async fn setup(site: Site) -> Setup {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let proxy = start_with(|s| s.sites = vec![site], up.clone(), Scripted::new(), true).await;
    Setup { proxy, up, app }
}

impl Setup {
    async fn connect(&self) -> Conn {
        let t = TcpStream::connect(self.proxy.http_addr().unwrap())
            .await
            .unwrap();
        Conn { t, buf: Vec::new() }
    }

    fn app_requests(&self) -> usize {
        self.app.requests.load(Ordering::SeqCst)
    }
}

struct Conn {
    t: TcpStream,
    buf: Vec<u8>,
}

fn complete(buf: &[u8]) -> Option<usize> {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
    let len = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .map_or(0, |v| v.trim().parse::<usize>().unwrap());
    (buf.len() >= end + len).then_some(end + len)
}

impl Conn {
    async fn send(&mut self, bytes: &[u8]) {
        self.t.write_all(bytes).await.unwrap();
        self.t.flush().await.unwrap();
    }

    async fn response(&mut self) -> Option<String> {
        let mut chunk = [0u8; 8192];
        loop {
            if let Some(n) = complete(&self.buf) {
                let r: Vec<u8> = self.buf.drain(..n).collect();
                return Some(String::from_utf8_lossy(&r).into_owned());
            }
            let read = tokio::time::timeout(Duration::from_secs(5), self.t.read(&mut chunk));
            match read.await {
                Ok(Ok(n)) if n > 0 => self.buf.extend_from_slice(&chunk[..n]),
                _ => return None,
            }
        }
    }

    async fn send_late_body(&mut self, req: &[u8]) {
        let end = req.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        self.send(&req[..end]).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.send(&req[end..]).await;
    }

    async fn closed(&mut self) -> bool {
        self.response().await.is_none() && self.buf.is_empty()
    }
}

fn closes(r: &str) -> bool {
    r.to_ascii_lowercase().contains("\r\nconnection: close\r\n")
}

fn post(extra: &str, len: usize) -> Vec<u8> {
    let mut v =
        format!("POST /up HTTP/1.1\r\nHost: {HOST}\r\n{extra}Content-Length: {len}\r\n\r\n")
            .into_bytes();
    v.extend(std::iter::repeat_n(b'x', len));
    v
}

fn chunked(sizes: &[usize], trailer: &str) -> Vec<u8> {
    let mut v = format!("POST /up HTTP/1.1\r\nHost: {HOST}\r\nTransfer-Encoding: chunked\r\n\r\n")
        .into_bytes();
    for n in sizes {
        v.extend(format!("{n:x}\r\n").bytes());
        v.extend(std::iter::repeat_n(b'x', *n));
        v.extend(b"\r\n");
    }
    v.extend(format!("0\r\n{trailer}\r\n").bytes());
    v
}

fn authed_get(path: &str) -> Vec<u8> {
    let auth = encode("bob", b"pw");
    format!("GET {path} HTTP/1.1\r\nHost: {HOST}\r\nAuthorization: {auth}\r\n\r\n").into_bytes()
}

#[tokio::test]
async fn an_early_401_after_a_small_body_keeps_the_connection_for_the_next_request() {
    let s = setup(protected()).await;
    let mut c = s.connect().await;
    let bodies = [
        post("", SMALL),
        chunked(&[1000, 3000], "x-sum: 1\r\n"),
        chunked(&[64 * 1024; 4], ""),
    ];
    for (i, body) in bodies.iter().enumerate() {
        c.send(body).await;
        let r = c.response().await.expect("the 401");
        assert!(r.starts_with("HTTP/1.1 401"), "{r}");
        assert!(!closes(&r), "{r}");
        c.send(&authed_get("/next")).await;
        let r = c.response().await.expect("the next answer");
        assert!(
            r.starts_with("HTTP/1.1 200") && r.contains("GET /next"),
            "{r}"
        );
        assert_eq!(s.app_requests(), i + 1);
    }
}

#[tokio::test]
async fn other_early_answers_keep_the_connection_too() {
    let mut maintenance = plain_site("blog", &[HOST]);
    maintenance.options.maintenance = true;
    let mut small = plain_site("blog", &[HOST]);
    small.options.max_body_size = Some(1000);
    let cases: [(&str, Site, &str); 5] = [
        ("too large", small, "413"),
        ("deny", plain_site("blog", &[HOST]), "403"),
        ("no site", plain_site("blog", &["other.example.com"]), "404"),
        ("maintenance", maintenance, "503"),
        ("app gone", plain_site("blog", &[HOST]), "50"),
    ];
    for (name, site, want) in cases {
        let s = setup(site).await;
        match name {
            "deny" => {
                s.up.denied
                    .lock()
                    .unwrap()
                    .push("127.0.0.1".parse().unwrap())
            }
            "app gone" => *s.up.refuse.lock().unwrap() = Some(Refused::Down),
            _ => {}
        }
        let mut c = s.connect().await;
        for _ in 0..2 {
            c.send_late_body(&post("", SMALL)).await;
            let r = c.response().await.expect(name);
            assert!(r.starts_with(&format!("HTTP/1.1 {want}")), "{name}: {r}");
            assert!(!closes(&r), "{name}: {r}");
        }
        assert_eq!(s.app_requests(), 0, "{name}");
    }
}

#[tokio::test]
async fn a_rate_limited_post_keeps_the_connection() {
    let mut site = plain_site("blog", &[HOST]);
    site.options.rate_limit = std::num::NonZeroU32::new(1);
    let s = setup(site).await;
    let mut c = s.connect().await;
    let mut limited = 0;
    for _ in 0..6 {
        c.send_late_body(&post("", SMALL)).await;
        let r = c
            .response()
            .await
            .expect("an answer on the same connection");
        assert!(!closes(&r), "{r}");
        limited += usize::from(r.starts_with("HTTP/1.1 429"));
    }
    assert!(limited > 0);
}

#[tokio::test]
async fn a_declared_body_over_256_kib_is_not_read_and_the_answer_closes() {
    let s = setup(protected()).await;
    let mut c = s.connect().await;
    let started = Instant::now();
    c.send(&post("", OVER)[..1024]).await;
    let r = c.response().await.expect("the 401");
    assert!(started.elapsed() < Duration::from_millis(900));
    assert!(r.starts_with("HTTP/1.1 401") && closes(&r), "{r}");
    assert!(c.closed().await);
}

#[tokio::test]
async fn a_chunked_body_over_256_kib_closes_cleanly() {
    let s = setup(protected()).await;
    let c = s.connect().await;
    let (mut rd, mut wr) = c.t.into_split();
    let body = chunked(&[16 * 1024; OVER / (16 * 1024)], "");
    let writer = tokio::spawn(async move {
        let _ = wr.write_all(&body).await;
        wr
    });
    let mut out = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(8), rd.read_to_end(&mut out)).await;
    let r = String::from_utf8_lossy(&out);
    assert!(read.unwrap().is_ok(), "a clean close, not a reset: {r}");
    assert!(r.starts_with("HTTP/1.1 401") && closes(&r), "{r}");
    assert_eq!(r.matches("HTTP/1.1").count(), 1, "{r}");
    drop(writer.await);
}

#[tokio::test]
async fn a_cut_off_body_is_never_read_as_the_next_request() {
    let mut cut = post("", 1000);
    cut.truncate(cut.len() - 500);
    let mut in_chunk = chunked(&[16], "");
    in_chunk.truncate(in_chunk.len() - 15);
    let mut between = chunked(&[3, 3], "");
    between.truncate(between.len() - 10);
    for (name, head) in [
        ("length", cut),
        ("in a chunk", in_chunk),
        ("between chunks", between),
    ] {
        let s = setup(protected()).await;
        let mut c = s.connect().await;
        let started = Instant::now();
        c.send(&head).await;
        let r = c.response().await.expect(name);
        let took = started.elapsed();
        assert!(r.starts_with("HTTP/1.1 401") && closes(&r), "{name}: {r}");
        assert!(took >= Duration::from_millis(900), "{name}: {took:?}");
        assert!(took < Duration::from_secs(3), "{name}: {took:?}");
        c.send(&authed_get("/secret")).await;
        assert!(c.closed().await, "{name}");
        assert_eq!(s.app_requests(), 0, "{name}");
    }
}

#[tokio::test]
async fn a_request_inside_a_body_is_only_body() {
    let s = setup(protected()).await;
    let mut c = s.connect().await;
    let hidden = authed_get("/hidden");
    let mut req = format!(
        "POST /up HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: {}\r\n\r\n",
        hidden.len()
    )
    .into_bytes();
    req.extend(&hidden);
    c.send(&req).await;
    let r = c.response().await.unwrap();
    assert!(r.starts_with("HTTP/1.1 401") && !closes(&r), "{r}");
    c.send(&authed_get("/real")).await;
    let r = c.response().await.unwrap();
    assert!(r.contains("GET /real") && !r.contains("/hidden"), "{r}");
    assert_eq!(s.app_requests(), 1);
}

#[tokio::test]
async fn pipelined_requests_after_an_early_answer_are_answered_in_order() {
    let s = setup(protected()).await;
    let mut c = s.connect().await;
    let mut both = post("", SMALL);
    both.extend(authed_get("/second"));
    c.send(&both).await;
    let first = c.response().await.unwrap();
    assert!(
        first.starts_with("HTTP/1.1 401") && !closes(&first),
        "{first}"
    );
    let second = c.response().await.unwrap();
    assert!(
        second.starts_with("HTTP/1.1 200") && second.contains("GET /second"),
        "{second}"
    );
    assert_eq!(s.app_requests(), 1);
}

#[tokio::test]
async fn a_slow_body_gets_its_answer_within_about_a_second_and_a_clean_close() {
    let s = setup(protected()).await;
    let c = s.connect().await;
    let (mut rd, mut wr) = c.t.into_split();
    let head = format!("POST /up HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: 100\r\n\r\n");
    wr.write_all(head.as_bytes()).await.unwrap();
    let writer = tokio::spawn(async move {
        for _ in 0..30 {
            if wr.write_all(b"x").await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    });
    let started = Instant::now();
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut first = None;
    let ended = loop {
        match tokio::time::timeout(Duration::from_secs(15), rd.read(&mut chunk)).await {
            Ok(Ok(0)) => break true,
            Ok(Ok(n)) => out.extend_from_slice(&chunk[..n]),
            _ => break false,
        }
        if first.is_none() && complete(&out).is_some() {
            first = Some(started.elapsed());
        }
    };
    let r = String::from_utf8_lossy(&out);
    assert!(ended, "a clean close, not a reset: {r}");
    assert!(r.starts_with("HTTP/1.1 401") && closes(&r), "{r}");
    let first = first.expect("the whole answer");
    assert!(
        first >= Duration::from_millis(900) && first < Duration::from_secs(3),
        "{first:?}"
    );
    writer.abort();
}

#[tokio::test]
async fn expect_100_continue_gets_no_continue_and_the_connection_closes() {
    let s = setup(protected()).await;
    let mut c = s.connect().await;
    let head = format!(
        "POST /up HTTP/1.1\r\nHost: {HOST}\r\nExpect: 100-continue\r\nContent-Length: {SMALL}\r\n\r\n"
    );
    c.send(head.as_bytes()).await;
    let r = c.response().await.unwrap();
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert!(c.closed().await);
}

#[tokio::test]
async fn an_http_1_0_post_still_closes_after_its_answer() {
    let s = setup(protected()).await;
    let mut c = s.connect().await;
    let mut req = post("", SMALL);
    req.splice(
        ..b"POST /up HTTP/1.1".len(),
        b"POST /up HTTP/1.0".iter().copied(),
    );
    c.send(&req).await;
    let r = c.response().await.unwrap();
    assert!(r.starts_with("HTTP/1.0 401"), "{r}");
    assert!(c.closed().await);
}

#[tokio::test]
async fn a_forwarded_post_keeps_the_connection_as_before() {
    let s = setup(protected()).await;
    let mut c = s.connect().await;
    let auth = format!("Authorization: {}\r\n", encode("bob", b"pw"));
    for _ in 0..2 {
        c.send(&post(&auth, SMALL)).await;
        let r = c.response().await.unwrap();
        assert!(r.starts_with("HTTP/1.1 200") && !closes(&r), "{r}");
        assert!(r.contains(&format!("body-len: {SMALL}")), "{r}");
    }
    assert_eq!(s.app_requests(), 2);
}

async fn early_app() -> std::net::SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut t, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && t.read(&mut b).await.is_ok_and(|n| n == 1) {
                    head.push(b[0]);
                }
                let _ = t
                    .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\n\r\nno")
                    .await;
                tokio::time::sleep(Duration::from_secs(3)).await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn a_post_the_app_answers_without_reading_behaves_as_before() {
    let s = setup(plain_site("blog", &[HOST])).await;
    s.up.serve("blog", early_app().await);
    let mut c = s.connect().await;
    c.send(&post("", SMALL)).await;
    let r = c.response().await.unwrap();
    assert!(r.starts_with("HTTP/1.1 401") && !closes(&r), "{r}");
    c.send(format!("GET / HTTP/1.1\r\nHost: {HOST}\r\n\r\n").as_bytes())
        .await;
    assert!(
        c.response().await.is_some(),
        "the same connection answers again"
    );
}
