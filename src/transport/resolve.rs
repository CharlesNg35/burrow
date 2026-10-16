use std::io;
use std::net::SocketAddr;

use tokio::net::lookup_host;

pub async fn resolve(host_port: &str) -> io::Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = lookup_host(host_port).await?.collect();
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "host name has no addresses",
        ));
    }
    Ok(addrs)
}
