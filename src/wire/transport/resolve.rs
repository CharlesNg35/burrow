use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

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

pub struct Known(pub Vec<SocketAddr>);

impl Resolver for Known {
    fn resolve<'a>(&'a self, _: &'a str) -> Resolved<'a> {
        Box::pin(std::future::ready(Ok(self.0.clone())))
    }
}

pub struct Recording<'a> {
    inner: &'a dyn Resolver,
    answer: Mutex<Option<Vec<SocketAddr>>>,
}

impl<'a> Recording<'a> {
    pub fn new(inner: &'a dyn Resolver) -> Recording<'a> {
        Recording {
            inner,
            answer: Mutex::new(None),
        }
    }

    pub fn answer(self) -> Option<Vec<SocketAddr>> {
        self.answer.into_inner().unwrap_or_else(|e| e.into_inner())
    }
}

impl Resolver for Recording<'_> {
    fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
        Box::pin(async move {
            let addrs = self.inner.resolve(host_port).await?;
            *self.answer.lock().unwrap_or_else(|e| e.into_inner()) = Some(addrs.clone());
            Ok(addrs)
        })
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
