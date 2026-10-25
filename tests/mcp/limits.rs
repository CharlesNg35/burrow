use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::common::admin::world;
use super::http::{modern, start};
use super::visitor;

fn head(port: u16, token: &str, len: usize) -> String {
    format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/list\r\nConnection: close\r\nContent-Length: {len}\r\n\r\n"
    )
}

async fn answer(s: &mut TcpStream, within: Duration) -> (String, Duration) {
    let started = Instant::now();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(within, s.read_to_end(&mut out)).await;
    (
        String::from_utf8_lossy(&out).into_owned(),
        started.elapsed(),
    )
}

#[tokio::test]
async fn the_ninth_connection_is_turned_away_while_eight_stay_served() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let h = start(&v, &["--no-print-token"]).await;
    let token = std::fs::read_dir(v.sessions())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "token"))
        .map(|p| std::fs::read_to_string(p).unwrap().trim().to_string())
        .unwrap();
    let body = modern("tools/list", "2026-07-28");
    let full = head(h.port, &token, body.len()) + &body;
    let (first, rest) = full.split_at(20);
    let mut held = Vec::new();
    for _ in 0..burrow::mcp::http::MAX_OPEN {
        let mut s = TcpStream::connect(("127.0.0.1", h.port)).await.unwrap();
        s.write_all(first.as_bytes()).await.unwrap();
        held.push(s);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut over = TcpStream::connect(("127.0.0.1", h.port)).await.unwrap();
    let _ = over.write_all(full.as_bytes()).await;
    let (got, took) = answer(&mut over, Duration::from_secs(3)).await;
    assert!(
        got.is_empty(),
        "a connection over the cap was served: {got}"
    );
    assert!(took < Duration::from_secs(2), "{took:?}");
    for s in &mut held {
        s.write_all(rest.as_bytes()).await.unwrap();
    }
    for mut s in held {
        let (got, _) = answer(&mut s, Duration::from_secs(10)).await;
        assert!(
            got.starts_with("HTTP/1.1 200"),
            "a held connection was not served: {got}"
        );
    }
    let mut again = TcpStream::connect(("127.0.0.1", h.port)).await.unwrap();
    again.write_all(full.as_bytes()).await.unwrap();
    let (got, _) = answer(&mut again, Duration::from_secs(10)).await;
    assert!(got.starts_with("HTTP/1.1 200"), "{got}");
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn half_a_header_is_cut_after_the_header_timeout() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let h = start(&v, &["--no-print-token"]).await;
    let mut s = TcpStream::connect(("127.0.0.1", h.port)).await.unwrap();
    s.write_all(format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n", h.port).as_bytes())
        .await
        .unwrap();
    let (got, took) = answer(&mut s, Duration::from_secs(12)).await;
    assert!(got.is_empty() || got.starts_with("HTTP/1.1 408"), "{got}");
    let limit = burrow::mcp::http::HEADER_TIMEOUT;
    assert!(
        took >= limit - Duration::from_secs(1) && took < limit + Duration::from_secs(3),
        "half a header was cut after {took:?}"
    );
    let _ = w.stop.send(true);
}
