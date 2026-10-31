pub mod client;
pub mod find;
pub mod open;
pub mod store;
pub mod token;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::base::failure::Failure;

pub async fn bind(port: u16) -> Result<TcpListener, Failure> {
    TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .map_err(|e| {
            Failure::new(format!("cannot listen on 127.0.0.1:{port}: {e}"))
                .hint("pick another --port, or 0 for a free one")
        })
}

const ACCEPT_RETRY: Duration = Duration::from_millis(50);

pub async fn accept_permitted(
    listener: &TcpListener,
    open: &Arc<Semaphore>,
) -> (TcpStream, OwnedSemaphorePermit) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(ACCEPT_RETRY).await;
            continue;
        };
        if let Ok(permit) = open.clone().try_acquire_owned() {
            return (stream, permit);
        }
    }
}
