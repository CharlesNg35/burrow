use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use ppp::{HeaderResult, PartialResult};
use tokio::io::{AsyncRead, AsyncReadExt};

pub const MAX_HEADER: usize = 2048;
pub const MAX_V1: usize = 107;
const V2_SIGNATURE: &[u8; 12] = b"\r\n\r\n\0\r\nQUIT\n";
const V2_FIXED: usize = 16;

fn v2_total(buf: &[u8]) -> Option<usize> {
    if buf.len() < V2_FIXED || !buf.starts_with(V2_SIGNATURE) {
        return None;
    }
    Some(V2_FIXED + usize::from(u16::from_be_bytes([buf[14], buf[15]])))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    More,
    Header {
        len: usize,
        source: Option<SocketAddr>,
    },
    Invalid,
}

pub fn parse(buf: &[u8]) -> Parsed {
    match v2_total(buf) {
        Some(total) if total > MAX_HEADER => return Parsed::Invalid,
        Some(total) if buf.len() < total => return Parsed::More,
        _ => {}
    }
    let header = HeaderResult::parse(buf);
    if header.is_incomplete() {
        let v1_line_done = buf.starts_with(b"PROXY")
            && (buf.len() >= MAX_V1 || buf.windows(2).any(|w| w == b"\r\n"));
        return if v1_line_done || buf.len() >= MAX_HEADER {
            Parsed::Invalid
        } else {
            Parsed::More
        };
    }
    let (len, source) = match header {
        HeaderResult::V1(Ok(h)) => (h.header.len(), v1_source(&h.addresses)),
        HeaderResult::V2(Ok(h)) => (h.len(), v2_source(&h)),
        _ => return Parsed::Invalid,
    };
    if len > MAX_HEADER || len > buf.len() {
        return Parsed::Invalid;
    }
    Parsed::Header { len, source }
}

fn v1_source(a: &ppp::v1::Addresses) -> Option<SocketAddr> {
    match a {
        ppp::v1::Addresses::Tcp4(a) => Some(SocketAddr::from((a.source_address, a.source_port))),
        ppp::v1::Addresses::Tcp6(a) => Some(SocketAddr::from((a.source_address, a.source_port))),
        ppp::v1::Addresses::Unknown => None,
    }
}

fn v2_source(h: &ppp::v2::Header<'_>) -> Option<SocketAddr> {
    if h.command == ppp::v2::Command::Local || h.protocol != ppp::v2::Protocol::Stream {
        return None;
    }
    match &h.addresses {
        ppp::v2::Addresses::IPv4(a) => Some(SocketAddr::from((a.source_address, a.source_port))),
        ppp::v2::Addresses::IPv6(a) => Some(SocketAddr::from((a.source_address, a.source_port))),
        _ => None,
    }
}

pub struct Read {
    pub source: Option<SocketAddr>,
    pub rest: Vec<u8>,
}

pub async fn read_proxy_header<S: AsyncRead + Unpin>(
    io: &mut S,
    limit: Duration,
) -> io::Result<Read> {
    let work = async {
        let mut buf = Vec::with_capacity(MAX_HEADER);
        let mut chunk = [0u8; MAX_HEADER];
        loop {
            let want = MAX_HEADER - buf.len();
            let n = io.read(&mut chunk[..want.max(1)]).await?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            buf.extend_from_slice(&chunk[..n]);
            match parse(&buf) {
                Parsed::More => continue,
                Parsed::Invalid => return Err(invalid()),
                Parsed::Header { len, source } => {
                    return Ok(Read {
                        source,
                        rest: buf.split_off(len),
                    });
                }
            }
        }
    };
    tokio::time::timeout(limit, work)
        .await
        .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "the connection did not start with a valid PROXY header",
    )
}

#[cfg(test)]
mod tests;
