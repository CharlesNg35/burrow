pub mod find;
pub mod open;
pub mod store;
pub mod token;

use std::net::SocketAddr;

use tokio::net::TcpListener;

use crate::base::failure::Failure;

pub async fn bind(port: u16) -> Result<TcpListener, Failure> {
    TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .map_err(|e| {
            Failure::new(format!("cannot listen on 127.0.0.1:{port}: {e}"))
                .hint("pick another --port, or 0 for a free one")
        })
}
