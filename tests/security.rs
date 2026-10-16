mod common;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use burrow::client::State;
use burrow::config::model::{Cipher, Protocol};
use burrow::noise::{Initiator, MSG2_LEN};
use burrow::protocol::{ChannelKind, Hello, Preamble};
use burrow::secret::Secret;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn first_flight(name: &str, secret: &Secret, kind: ChannelKind) -> (Initiator, Vec<u8>) {
    let p = Preamble {
        kind,
        cipher: Cipher::Chachapoly,
        plaintext: false,
        name: name.into(),
    };
    let (init, msg1) = Initiator::start(p.cipher, secret, &p.prologue());
    let mut bytes = p.encode();
    bytes.extend_from_slice(&msg1);
    (init, bytes)
}

async fn reply_len(addr: SocketAddr, bytes: &[u8]) -> (usize, Duration) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.set_nodelay(true).unwrap();
    let start = Instant::now();
    s.write_all(bytes).await.unwrap();
    let mut buf = [0u8; 256];
    let mut total = 0;
    loop {
        match tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf[total..])).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(n)) => {
                total += n;
                if total >= MSG2_LEN {
                    break;
                }
            }
        }
    }
    (total, start.elapsed())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_secret_gets_no_reply() {
    let srv = start_server(server_settings(vec![Secret::generate()])).await;
    let (_, bytes) = first_flight("home", &Secret::generate(), ChannelKind::Control);
    let (n, _) = reply_len(srv.addr, &bytes).await;
    assert_eq!(n, 0, "server must close without sending anything");
    assert!(srv.shared.session_names().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_secret_client_reports_rejection() {
    let srv = start_server(server_settings(vec![Secret::generate()])).await;
    let mut cli = start_client(client_settings(srv.addr, Secret::generate())).await;
    wait_state(&mut cli.state, State::Rejected).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_name_looks_like_wrong_secret() {
    let srv = start_server(server_settings(vec![Secret::generate()])).await;
    let mut known = Vec::new();
    let mut unknown = Vec::new();
    for _ in 0..30 {
        let (_, b) = first_flight("home", &Secret::generate(), ChannelKind::Control);
        reply_len(srv.addr, &b).await;
    }
    for _ in 0..300 {
        let (_, a) = first_flight("home", &Secret::generate(), ChannelKind::Control);
        let (_, b) = first_flight("nobody", &Secret::generate(), ChannelKind::Control);
        let (na, ta) = reply_len(srv.addr, &a).await;
        let (nb, tb) = reply_len(srv.addr, &b).await;
        assert_eq!(
            (na, nb),
            (0, 0),
            "identical response: connection closed without bytes"
        );
        known.push(ta);
        unknown.push(tb);
    }
    known.sort();
    unknown.sort();
    let (mk, mu) = (known[known.len() / 2], unknown[unknown.len() / 2]);
    let ratio = mk.as_secs_f64() / mu.as_secs_f64();
    eprintln!("median wrong-secret {mk:?}, unknown-name {mu:?}, ratio {ratio:.3}");
    assert!(
        (0.6..1.66).contains(&ratio),
        "timing differs too much: {mk:?} vs {mu:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replayed_first_message_cannot_open_a_session() {
    let secret = Secret::generate();
    let srv = start_server(server_settings(vec![secret.clone()])).await;
    let (_, bytes) = first_flight("home", &secret, ChannelKind::Control);

    let (n, _) = reply_len(srv.addr, &bytes).await;
    assert_eq!(n, MSG2_LEN, "the original is answered");

    let mut s = TcpStream::connect(srv.addr).await.unwrap();
    s.write_all(&bytes).await.unwrap();
    let mut msg2 = [0u8; MSG2_LEN];
    s.read_exact(&mut msg2).await.unwrap();
    let forged: Vec<u8> = [&[0u8, 18][..], &[0x55; 18][..]].concat();
    s.write_all(&forged).await.unwrap();
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut rest)).await;
    assert!(rest.is_empty(), "no welcome for a replayed handshake");
    assert!(srv.shared.session_names().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn truncated_and_modified_handshakes_are_dropped() {
    let secret = Secret::generate();
    let echo = echo_server().await;
    let mut settings = server_settings(vec![secret.clone()]);
    settings.handshake_timeout = Duration::from_millis(500);
    settings
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let srv = start_server(settings).await;
    let (_, good) = first_flight("home", &secret, ChannelKind::Control);

    for cut in [1, 6, 7, 10, good.len() - 1] {
        let (n, t) = reply_len(srv.addr, &good[..cut]).await;
        assert_eq!(n, 0);
        assert!(
            t < Duration::from_secs(3),
            "truncated handshake closed by the timeout"
        );
    }
    for i in [0, 4, 5, 6, 8, good.len() - 20, good.len() - 1] {
        let mut bad = good.clone();
        bad[i] ^= 0x01;
        let (n, _) = reply_len(srv.addr, &bad).await;
        assert_eq!(n, 0, "flipping byte {i} must be rejected");
    }
    for junk in [
        &b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"[..],
        &[0u8; 200][..],
        &[0xffu8; 7][..],
    ] {
        let (n, _) = reply_len(srv.addr, junk).await;
        assert_eq!(n, 0);
    }
    assert!(srv.shared.session_names().is_empty());

    let mut cli = start_client(client_settings(srv.addr, secret)).await;
    wait_state(&mut cli.state, State::Connected).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn data_channel_with_wrong_token_is_not_pooled() {
    let secret = Secret::generate();
    let srv = start_server(server_settings(vec![secret.clone()])).await;
    let mut cli = start_client(client_settings(srv.addr, secret.clone())).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 2).await;
    let before = srv.shared.idle_channels("home");

    let s = TcpStream::connect(srv.addr).await.unwrap();
    let p = Preamble {
        kind: ChannelKind::Data,
        cipher: Cipher::Chachapoly,
        plaintext: false,
        name: "home".into(),
    };
    let ch = burrow::handshake::initiate(
        s,
        &p,
        &secret,
        &Hello::Data {
            version: 1,
            token: [0u8; 16],
        },
    )
    .await;
    let mut ch = ch.expect("handshake itself succeeds");
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(3), ch.stream.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(n, 0, "server closes a data channel with a stale token");
    assert_eq!(srv.shared.idle_channels("home"), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rotation_accepts_old_and_new_secret() {
    let old = Secret::generate();
    let new = Secret::generate();
    let echo = echo_server().await;
    let mut settings = server_settings(vec![old.clone(), new.clone()]);
    settings
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let srv = start_server(settings).await;
    for s in [old, new] {
        let mut cli = start_client(client_settings(srv.addr, s)).await;
        wait_state(&mut cli.state, State::Connected).await;
        let shared = srv.shared.clone();
        wait_for(|| shared.idle_channels("home") >= 1).await;
        assert_eq!(roundtrip(srv.services["web"], b"rotate").await, b"rotate");
        cli.stop.send(true).unwrap();
        cli.task.await.unwrap();
        let shared = srv.shared.clone();
        wait_for(|| shared.session_names().is_empty()).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handshake_flood_is_capped_and_legit_client_gets_in() {
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.max_handshakes = 8;
    settings.handshake_timeout = Duration::from_millis(800);
    let srv = start_server(settings).await;

    let mut flood = Vec::new();
    for _ in 0..200 {
        flood.push(TcpStream::connect(srv.addr).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut closed_fast = 0;
    for s in &mut flood {
        let mut b = [0u8; 1];
        if let Ok(Ok(0)) = tokio::time::timeout(Duration::from_millis(5), s.read(&mut b)).await {
            closed_fast += 1;
        }
    }
    assert!(
        closed_fast >= 180,
        "connections over the cap are closed at once ({closed_fast})"
    );

    let start = Instant::now();
    let mut cli = start_client(client_settings(srv.addr, secret)).await;
    wait_state(&mut cli.state, State::Connected).await;
    assert!(start.elapsed() < Duration::from_secs(5));
    drop(flood);
}

#[test]
fn secrets_never_appear_in_debug_output() {
    let s = Secret::generate();
    let settings = client_settings("127.0.0.1:1".parse().unwrap(), s.clone());
    let dbg = format!("{settings:?}");
    assert!(!dbg.contains(&s.encode()));
    let proxy = burrow::proxy::ProxyConfig::parse("socks5://user:hunter2@p:1080").unwrap();
    assert!(!format!("{proxy:?}").contains("hunter2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn silent_client_is_closed_after_heartbeat_timeout() {
    let secret = Secret::generate();
    let srv = start_server(server_settings(vec![secret.clone()])).await;
    let s = TcpStream::connect(srv.addr).await.unwrap();
    let p = Preamble {
        kind: ChannelKind::Control,
        cipher: Cipher::Chachapoly,
        plaintext: false,
        name: "home".into(),
    };
    let mut ch = burrow::handshake::initiate(s, &p, &secret, &Hello::Control { version: 1 })
        .await
        .expect("handshake");
    let shared = srv.shared.clone();
    wait_for(|| shared.session_names().len() == 1).await;
    let start = Instant::now();
    let closed = loop {
        if start.elapsed() > Duration::from_secs(10) {
            break false;
        }
        match tokio::time::timeout(Duration::from_secs(2), ch.reader.recv(&mut ch.stream)).await {
            Ok(Ok(Some(_))) => continue,
            Ok(Ok(None)) | Ok(Err(_)) => break true,
            Err(_) => continue,
        }
    };
    assert!(
        closed,
        "server must close the socket of a client that never answers pings"
    );
    assert!(
        start.elapsed() < Duration::from_secs(8),
        "closed after about 3 heartbeat intervals"
    );
    assert!(srv.shared.session_names().is_empty());
}
