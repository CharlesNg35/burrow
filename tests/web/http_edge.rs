use std::net::SocketAddr;
use std::time::Duration;

use burrow::wire::secret::Secret;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_websockets::{ClientBuilder, Message, ServerBuilder};

use crate::common::*;
use crate::kit::*;
use crate::sites::with_files;

async fn framed_app() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut s = BufReader::new(s);
                while let Some(answer) = one_request(&mut s).await {
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{answer}",
                        answer.len()
                    );
                    if s.get_mut().write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

async fn one_request(s: &mut BufReader<tokio::net::TcpStream>) -> Option<String> {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if s.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        head.push_str(&line.to_ascii_lowercase());
        if line == "\r\n" {
            break;
        }
    }
    let mut body = 0usize;
    if head.contains("transfer-encoding: chunked\r\n") {
        loop {
            let mut size = String::new();
            s.read_line(&mut size).await.ok()?;
            let n = usize::from_str_radix(size.trim().split(';').next()?, 16).ok()?;
            if n == 0 {
                let mut end = String::new();
                s.read_line(&mut end).await.ok()?;
                break;
            }
            let mut chunk = vec![0u8; n + 2];
            s.read_exact(&mut chunk).await.ok()?;
            body += n;
        }
    } else if let Some(n) = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length: "))
        .and_then(|n| n.trim().parse::<usize>().ok())
    {
        let mut b = vec![0u8; n];
        s.read_exact(&mut b).await.ok()?;
        body = n;
    }
    Some(format!("{head}body-len: {body}\n"))
}

struct Edge {
    _t: Tunnel,
    web: SocketAddr,
    cert: Cert,
    _dir: tempfile::TempDir,
}

async fn edge(app: SocketAddr) -> Edge {
    let dir = tempfile::tempdir().unwrap();
    let c = cert(&["blog.example.com"]);
    let secret = Secret::generate();
    let mut s = web_server(
        &secret,
        &[("blog", site(&["home"], app), &["blog.example.com"])],
    );
    with_files(&mut s, "blog", &c, dir.path());
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let web = t.server.web_addr.unwrap();
    Edge {
        _t: t,
        web,
        cert: c,
        _dir: dir,
    }
}

async fn raw(e: &Edge, req: &[u8]) -> String {
    let mut t = visit(e.web, "blog.example.com", &e.cert).await.unwrap();
    let _ = t.write_all(req).await;
    let _ = t.flush().await;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), t.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_reads_a_body_one_way_and_refuses_ambiguous_framing() {
    let e = edge(framed_app().await).await;
    let r = raw(
        &e,
        b"POST / HTTP/1.1\r\nHost: blog.example.com\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\nGET /next HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    let first = r.split("HTTP/1.1 ").nth(1).unwrap_or_default();
    assert!(first.starts_with("200"), "{r}");
    assert!(
        first.contains("post / http/1.1\r\n") && first.contains("body-len: 0\n"),
        "CL+TE is read as chunked: {r}"
    );
    assert!(
        !first.contains("content-length: 4\r\n"),
        "the conflicting length is dropped: {r}"
    );
    assert_eq!(
        first.matches("transfer-encoding: chunked\r\n").count(),
        1,
        "the app gets burrow's own framing: {r}"
    );
    assert!(
        r.to_ascii_lowercase().contains("connection: close\r\n") && !r.contains("get /next"),
        "the connection closes after an ambiguous request, so nothing after it reaches the app: {r}"
    );
    for (req, what) in [
        (
            &b"POST / HTTP/1.1\r\nHost: blog.example.com\r\nContent-Length: 4\r\nContent-Length: 5\r\n\r\nabcde"[..],
            "CL,CL",
        ),
        (
            &b"POST / HTTP/1.1\r\nHost: blog.example.com\r\nTransfer-Encoding: xchunked\r\n\r\n0\r\n\r\n"[..],
            "TE xchunked",
        ),
        (
            &b"POST / HTTP/1.0\r\nHost: blog.example.com\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"[..],
            "TE on HTTP/1.0",
        ),
        (
            &b"GET / HTTP/1.1\r\nHost: blog.example.com\r\nX-A: 1\r\n  folded\r\n\r\n"[..],
            "obs-fold",
        ),
    ] {
        let r = raw(&e, req).await;
        assert!(r.starts_with("HTTP/1.1 400"), "{what}: {r}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn heads_over_32_kib_or_100_headers_get_431_and_a_30_kib_head_passes() {
    let e = edge(framed_app().await).await;
    let first = |r: &str| r.lines().next().unwrap_or("(closed)").to_string();
    let mut big = b"GET / HTTP/1.1\r\nHost: blog.example.com\r\nX-Big: ".to_vec();
    big.extend(vec![b'a'; 33 * 1024]);
    big.extend(b"\r\n\r\n");
    let r = raw(&e, &big).await;
    assert!(r.starts_with("HTTP/1.1 431"), "{}", first(&r));
    let mut many = b"GET / HTTP/1.1\r\nHost: blog.example.com\r\n".to_vec();
    for i in 0..101 {
        many.extend(format!("X-{i}: 1\r\n").as_bytes());
    }
    many.extend(b"\r\n");
    let r = raw(&e, &many).await;
    assert!(r.starts_with("HTTP/1.1 431"), "{}", first(&r));
    let mut fine =
        b"GET / HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\nX-Big: ".to_vec();
    fine.extend(vec![b'a'; 30 * 1024]);
    fine.extend(b"\r\n\r\n");
    let r = raw(&e, &fine).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{}", first(&r));
}

async fn ws_echo() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            tokio::spawn(async move {
                let Ok((_, mut ws)) = ServerBuilder::new().accept(s).await else {
                    return;
                };
                while let Some(Ok(m)) = ws.next().await {
                    if m.is_close() || ws.send(m).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_apps_websocket_passes_through_termination_and_the_tunnel() {
    let e = edge(ws_echo().await).await;
    let t = visit(e.web, "blog.example.com", &e.cert).await.unwrap();
    let (mut ws, answer) = ClientBuilder::new()
        .uri("wss://blog.example.com/socket")
        .unwrap()
        .connect_on(t)
        .await
        .unwrap();
    assert_eq!(answer.status(), 101);
    for i in 0..50 {
        let m = format!("m{i}");
        ws.send(Message::text(m.clone())).await.unwrap();
        let got = ws.next().await.unwrap().unwrap();
        assert_eq!(got.as_text(), Some(m.as_str()));
    }
    let big = vec![7u8; 1024 * 1024];
    ws.send(Message::binary(big.clone())).await.unwrap();
    let got = ws.next().await.unwrap().unwrap();
    assert_eq!(&*got.into_payload(), big.as_slice());
    ws.close().await.unwrap();
}
