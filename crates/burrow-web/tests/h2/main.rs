mod basics;
#[path = "../common/mod.rs"]
mod common;
mod conformance;
mod kit;
mod to_app;

use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_websockets::ServerBuilder;

pub async fn ws_echo() -> SocketAddr {
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
