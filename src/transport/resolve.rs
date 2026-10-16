use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use tokio::net::lookup_host;

pub type Resolved<'a> = Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>>;

pub trait Resolver: Send + Sync {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a>;
}

pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
        Box::pin(resolve(host_port))
    }
}

pub fn system() -> Arc<dyn Resolver> {
    Arc::new(SystemResolver)
}

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
