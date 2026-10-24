mod common;

use std::net::SocketAddr;

use common::*;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_websockets::{ClientBuilder, Message, ServerBuilder};

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

#[tokio::test]
async fn a_websocket_passes_through_termination_with_small_and_large_frames() {
    let app = ws_echo().await;
    let (up, _) = Fake::new(4);
    up.serve("chat", app);
    let proxy = start(
        |s| s.sites = vec![https_site("chat", &["*.apps.example.com"])],
        up,
    )
    .await;
    let t = tls(proxy.https_addr().unwrap(), "chat.apps.example.com")
        .await
        .unwrap();
    let (mut ws, answer) = ClientBuilder::new()
        .uri("wss://chat.apps.example.com/socket")
        .unwrap()
        .connect_on(t)
        .await
        .unwrap();
    assert_eq!(answer.status(), 101);
    for i in 0..200 {
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
