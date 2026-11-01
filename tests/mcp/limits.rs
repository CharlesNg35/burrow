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
    let token = token_of(&v);
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

fn token_of(v: &super::Visitor) -> String {
    std::fs::read_dir(v.sessions())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "token"))
        .map(|p| std::fs::read_to_string(p).unwrap().trim().to_string())
        .unwrap()
}

#[tokio::test]
async fn trickling_bodies_are_cut_and_a_good_client_gets_in_after_them() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let h = start(&v, &["--no-print-token"]).await;
    let token = token_of(&v);
    let mut tricklers = Vec::new();
    for _ in 0..burrow::mcp::http::MAX_OPEN {
        let mut s = TcpStream::connect(("127.0.0.1", h.port)).await.unwrap();
        s.write_all(head(h.port, &token, 1000).as_bytes())
            .await
            .unwrap();
        s.write_all(b"{").await.unwrap();
        tricklers.push(s);
    }
    let started = Instant::now();
    let keep = tokio::spawn(async move {
        let mut cut = 0;
        for mut s in tricklers {
            let mut out = Vec::new();
            loop {
                if s.write_all(b" ").await.is_err() {
                    break;
                }
                match tokio::time::timeout(Duration::from_millis(500), s.read_to_end(&mut out))
                    .await
                {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
            let text = String::from_utf8_lossy(&out).into_owned();
            assert!(
                text.is_empty() || text.starts_with("HTTP/1.1 408"),
                "{text}"
            );
            cut += 1;
        }
        cut
    });
    let body = modern("tools/list", "2026-07-28");
    let full = head(h.port, &token, body.len()) + &body;
    let mut served = String::new();
    while started.elapsed() < Duration::from_secs(30) {
        let mut s = TcpStream::connect(("127.0.0.1", h.port)).await.unwrap();
        s.write_all(full.as_bytes()).await.unwrap();
        let (got, _) = answer(&mut s, Duration::from_secs(5)).await;
        if got.starts_with("HTTP/1.1 200") {
            served = got;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        served.starts_with("HTTP/1.1 200"),
        "a good client never got in"
    );
    let waited = started.elapsed();
    let limit = burrow::mcp::http::BODY_TIMEOUT;
    assert!(
        waited >= limit - Duration::from_secs(1),
        "the slots were free too early: {waited:?}"
    );
    assert!(waited < limit + Duration::from_secs(8), "{waited:?}");
    assert_eq!(keep.await.unwrap(), burrow::mcp::http::MAX_OPEN);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_refused_request_is_answered_and_closed_without_reading_its_body() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let h = start(&v, &["--no-print-token"]).await;
    let mut s = TcpStream::connect(("127.0.0.1", h.port)).await.unwrap();
    s.write_all(head(h.port, "wrong", 100_000).as_bytes())
        .await
        .unwrap();
    let (got, took) = answer(&mut s, Duration::from_secs(5)).await;
    assert!(got.starts_with("HTTP/1.1 401"), "{got}");
    assert!(
        got.to_ascii_lowercase().contains("connection: close"),
        "{got}"
    );
    assert!(
        took < Duration::from_secs(2),
        "the connection stayed open for the body: {took:?}"
    );
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn an_early_answer_to_a_large_body_ends_in_a_clean_close_not_a_reset() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let h = start(&v, &["--no-print-token"]).await;
    let addr = ([127, 0, 0, 1], h.port).into();
    let head = head(h.port, "wrong", burrow_io::linger::probe::BODY);
    let ended = burrow_io::linger::probe::send_large_body(addr, &head).await;
    assert!(ended.answer.starts_with("HTTP/1.1 401"), "{ended:?}");
    assert!(ended.broken.is_none(), "{ended:?}");
    let _ = w.stop.send(true);
}
