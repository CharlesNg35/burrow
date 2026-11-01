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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_writer_in_another_task_resumes_after_a_stall_while_pings_are_due() {
    use futures_util::StreamExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let total = 16 << 20;
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut conn = accept(Carrier::Plain(tcp), "/burrow").await.unwrap();
        conn.keep_alive(std::time::Duration::from_millis(300));
        let (mut rd, mut wr) = tokio::io::split(conn);
        tokio::spawn(async move {
            let mut buf = [0u8; 64];
            while rd.read(&mut buf).await.is_ok_and(|n| n > 0) {}
        });
        let chunk = vec![7u8; 64 * 1024];
        for _ in 0..total / chunk.len() {
            wr.write_all(&chunk).await.unwrap();
        }
        wr.flush().await.unwrap();
    });
    let tcp = TcpStream::connect(addr).await.unwrap();
    let (mut ws, _) = ClientBuilder::new()
        .uri(&format!("ws://{addr}/burrow"))
        .unwrap()
        .connect_on(tcp)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let mut got = 0;
    let read = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while got < total {
            match ws.next().await {
                Some(Ok(m)) if m.is_binary() => got += m.as_payload().len(),
                Some(Ok(_)) => {}
                _ => break,
            }
        }
    })
    .await;
    assert!(read.is_ok(), "got {got} of {total} bytes");
    tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("the writer finishes")
        .unwrap();
}

#[tokio::test]
async fn an_upgrade_after_an_empty_line_is_accepted_and_garbage_is_not() {
    for (sent, upgraded) in [
        (&b"\r\nGET /burrow HTTP/1.1\r\n"[..], true),
        (&b"\x01\x02 noise"[..], false),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            accept(Carrier::Plain(tcp), "/burrow").await.is_ok()
        });
        let mut tcp = TcpStream::connect(addr).await.unwrap();
        tcp.write_all(sent).await.unwrap();
        if upgraded {
            tcp.write_all(b"Host: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").await.unwrap();
        }
        let mut answer = [0u8; 12];
        tcp.read_exact(&mut answer).await.unwrap();
        let code = if upgraded {
            &b"HTTP/1.1 101"[..]
        } else {
            &b"HTTP/1.1 400"[..]
        };
        assert_eq!(&answer[..], code);
        drop(tcp);
        assert_eq!(server.await.unwrap(), upgraded);
    }
}

#[tokio::test]
async fn a_refused_upgrade_with_a_large_body_ends_in_a_clean_close_not_a_reset() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        accept(Carrier::Plain(tcp), "/burrow").await.is_err()
    });
    let head = format!(
        "POST /other HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n",
        burrow_io::linger::probe::BODY
    );
    let ended = burrow_io::linger::probe::send_large_body(addr, &head).await;
    assert!(ended.answer.starts_with("HTTP/1.1 404"), "{ended:?}");
    assert!(ended.broken.is_none(), "{ended:?}");
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

fn accepted(io: tokio::io::DuplexStream) -> Carrier {
    Carrier::Accepted(Box::new(Accepted {
        io: Box::new(io),
        local: "[::]:443".parse().unwrap(),
    }))
}

#[tokio::test]
async fn an_accepted_stream_upgrades_and_keeps_the_address_it_came_in_on() {
    use futures_util::{SinkExt, StreamExt};
    let (a, b) = tokio::io::duplex(1 << 16);
    let server = tokio::spawn(async move {
        let mut conn = accept(accepted(a), "/burrow").await.unwrap();
        assert_eq!(conn.local_addr().unwrap(), "[::]:443".parse().unwrap());
        let mut got = [0u8; 4];
        conn.read_exact(&mut got).await.unwrap();
        conn.write_all(b"pong").await.unwrap();
        conn.finish().await.unwrap();
        got
    });
    let (mut ws, _) = ClientBuilder::new()
        .uri("wss://tunnel.example.com/burrow")
        .unwrap()
        .connect_on(b)
        .await
        .unwrap();
    ws.send(Message::binary(b"ping".to_vec())).await.unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(&reply.into_payload()[..], b"pong");
    let end = ws.next().await;
    assert!(
        end.as_ref()
            .is_none_or(|m| m.as_ref().is_ok_and(Message::is_close)),
        "the server's finish closes the WebSocket: {end:?}"
    );
    assert_eq!(&server.await.unwrap(), b"ping");
}

#[tokio::test]
async fn an_upgraded_stream_keeps_the_message_and_control_frame_limits() {
    use futures_util::SinkExt;
    let (a, b) = tokio::io::duplex(1 << 20);
    let server = tokio::spawn(async move {
        let mut conn = serve_upgraded(accepted(a));
        let mut got = vec![0u8; WRITE_BATCH];
        conn.read_exact(&mut got).await.map(|_| ())?;
        conn.read_exact(&mut got[..1]).await.map(|_| ())
    });
    let mut ws = ClientBuilder::new().take_over(b);
    ws.send(Message::binary(vec![1u8; WRITE_BATCH]))
        .await
        .unwrap();
    let _ = ws.send(Message::binary(vec![1u8; WRITE_BATCH + 1])).await;
    assert!(server.await.unwrap().is_err(), "a message over the limit");
    let (a, b) = tokio::io::duplex(1 << 20);
    let server = tokio::spawn(async move {
        let mut conn = serve_upgraded(accepted(a));
        let mut got = [0u8; 4];
        conn.read_exact(&mut got).await
    });
    let mut ws = ClientBuilder::new().take_over(b);
    for _ in 0..=MAX_CONTROLS {
        if ws.send(Message::ping(Vec::new())).await.is_err() {
            break;
        }
    }
    let _ = ws.send(Message::binary(vec![7u8; 4])).await;
    assert!(server.await.unwrap().is_err(), "too many control frames");
}
