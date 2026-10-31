use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_rustls::rustls::server::Acceptor;

pub const MAX_HELLO: usize = 16 * 1024 + 5;
pub const HANDSHAKE_RECORD: u8 = 0x16;

#[derive(Debug, PartialEq, Eq)]
pub enum Peek {
    Hello(Hello),
    NotTls,
    TooBig,
    Closed,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Hello {
    pub name: Option<String>,
    pub alpn: Vec<Vec<u8>>,
    pub bytes: Vec<u8>,
}

const RECORD_HEADER: usize = 5;

fn record_len(data: &[u8]) -> Option<usize> {
    let len = data.get(3..RECORD_HEADER)?;
    Some(usize::from(u16::from_be_bytes([len[0], len[1]])) + RECORD_HEADER)
}

#[derive(Default)]
pub struct Peeker {
    acceptor: Acceptor,
    bytes: Vec<u8>,
    fed: usize,
}

impl Peeker {
    pub fn feed(&mut self, data: &[u8]) -> Option<Peek> {
        if data.is_empty() {
            return Some(Peek::Closed);
        }
        if self.bytes.len() + data.len() > MAX_HELLO {
            return Some(Peek::TooBig);
        }
        if self.bytes.first().unwrap_or(&data[0]) != &HANDSHAKE_RECORD {
            return Some(Peek::NotTls);
        }
        if self.bytes.is_empty() {
            let first = record_len(data).unwrap_or(0).min(MAX_HELLO);
            self.bytes.reserve_exact(first.max(data.len()));
        }
        self.bytes.extend_from_slice(data);
        while let Some(len) = record_len(&self.bytes[self.fed..]) {
            if self.bytes.len() - self.fed < len {
                return None;
            }
            if let Some(done) = self.read_record(len) {
                return Some(done);
            }
        }
        None
    }

    fn read_record(&mut self, len: usize) -> Option<Peek> {
        let mut rd = &self.bytes[self.fed..self.fed + len];
        self.fed += len;
        while !rd.is_empty() {
            match self.acceptor.read_tls(&mut rd) {
                Ok(0) | Err(_) => return Some(Peek::NotTls),
                Ok(_) => {}
            }
        }
        match self.acceptor.accept() {
            Ok(None) => None,
            Ok(Some(accepted)) => {
                let hello = accepted.client_hello();
                let name = hello.server_name().map(burrow_syntax::web::sni_name);
                let alpn = hello
                    .alpn()
                    .map(|list| list.map(<[u8]>::to_vec).collect())
                    .unwrap_or_default();
                Some(Peek::Hello(Hello {
                    name,
                    alpn,
                    bytes: std::mem::take(&mut self.bytes),
                }))
            }
            Err(_) => Some(Peek::NotTls),
        }
    }
}

pub async fn peek_sni<S: AsyncRead + Unpin>(s: &mut S, limit: Duration) -> io::Result<Peek> {
    let mut peeker = Peeker::default();
    let work = async {
        let mut chunk = [0u8; 4096];
        loop {
            let n = s.read(&mut chunk).await?;
            if let Some(done) = peeker.feed(&chunk[..n]) {
                return Ok(done);
            }
        }
    };
    tokio::time::timeout(limit, work)
        .await
        .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
}

#[cfg(test)]
mod tests;
