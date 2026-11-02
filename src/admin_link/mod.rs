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
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) if crate::wire::transport::tcp::gone_before_accept(&e) => continue,
            Err(_) => {
                tokio::time::sleep(ACCEPT_RETRY).await;
                continue;
            }
        };
        if let Ok(permit) = open.clone().try_acquire_owned() {
            return (stream, permit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_connection_reset_before_accept_does_not_delay_the_next_one() {
        let listener = bind(0).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let early = std::net::TcpStream::connect(addr).unwrap();
        socket2::SockRef::from(&early)
            .set_linger(Some(Duration::ZERO))
            .unwrap();
        drop(early);
        let _next = std::net::TcpStream::connect(addr).unwrap();
        let open = Arc::new(Semaphore::new(1));
        let started = tokio::time::Instant::now();
        let _ = accept_permitted(&listener, &open).await;
        assert!(started.elapsed() < ACCEPT_RETRY);
    }
}
