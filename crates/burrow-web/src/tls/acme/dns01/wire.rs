use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use hickory_proto::op::{Edns, Message, MessageType, Query};
use hickory_proto::rr::{Name, RecordType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

pub const QUERY_TIMEOUT: Duration = Duration::from_secs(3);
pub const UPDATE_TIMEOUT: Duration = Duration::from_secs(10);
pub const DNS_PORT: u16 = 53;
const PAYLOAD: u16 = 1232;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reach {
    pub redirect: BTreeMap<IpAddr, SocketAddr>,
    pub cloudflare_api: Option<String>,
    pub api_ca: Option<std::path::PathBuf>,
}

impl Reach {
    pub fn nameserver(&self, ip: IpAddr) -> SocketAddr {
        self.redirect
            .get(&ip)
            .copied()
            .unwrap_or(SocketAddr::new(ip, DNS_PORT))
    }
}

pub fn name(text: &str) -> Result<Name, String> {
    let mut n =
        Name::from_ascii(text.trim()).map_err(|e| format!("\"{text}\" is not a DNS name: {e}"))?;
    n.set_fqdn(true);
    Ok(n)
}

pub fn shown(n: &Name) -> String {
    let t = n.to_ascii();
    t.strip_suffix('.').unwrap_or(&t).to_ascii_lowercase()
}

pub fn question(name: &Name, kind: RecordType, recurse: bool) -> Message {
    let mut m = Message::query();
    m.metadata.recursion_desired = recurse;
    m.add_query(Query::query(name.clone(), kind));
    let mut edns = Edns::new();
    edns.set_max_payload(PAYLOAD);
    m.edns = Some(edns);
    m
}

pub fn reply_to(query: &Message, raw: &[u8]) -> Result<Message, String> {
    let m = Message::from_vec(raw).map_err(|e| format!("an unreadable answer ({e})"))?;
    if m.metadata.id != query.metadata.id
        || m.metadata.message_type != MessageType::Response
        || m.queries != query.queries
    {
        return Err("an answer that does not match the question".into());
    }
    Ok(m)
}

pub async fn ask(server: SocketAddr, query: &Message) -> Result<Message, Failed> {
    let bytes = query
        .to_vec()
        .map_err(|e| Failed::Bad(format!("cannot write the DNS question: {e}")))?;
    let raw = udp(server, &bytes, query.metadata.id, QUERY_TIMEOUT).await?;
    let bad = |e: String| Failed::Bad(format!("{server} sent {e}"));
    let m = reply_to(query, &raw).map_err(bad)?;
    if !m.metadata.truncation {
        return Ok(m);
    }
    let raw = tcp(server, &bytes, QUERY_TIMEOUT)
        .await
        .map_err(Failed::Silent)?;
    reply_to(query, &raw).map_err(bad)
}

fn unspecified(server: SocketAddr) -> SocketAddr {
    match server {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failed {
    NoRoute(String),
    Silent(String),
    Bad(String),
}

impl Failed {
    pub fn text(&self) -> &str {
        match self {
            Failed::NoRoute(t) | Failed::Silent(t) | Failed::Bad(t) => t,
        }
    }
}

impl From<Failed> for String {
    fn from(f: Failed) -> String {
        f.text().to_string()
    }
}

pub async fn udp(
    server: SocketAddr,
    bytes: &[u8],
    id: u16,
    limit: Duration,
) -> Result<Vec<u8>, Failed> {
    let route = |e: std::io::Error| Failed::NoRoute(format!("{server}: {e}"));
    let sock = UdpSocket::bind(unspecified(server)).await.map_err(route)?;
    sock.connect(server).await.map_err(route)?;
    sock.send(bytes).await.map_err(route)?;
    let mut buf = vec![0u8; 65535];
    let wait = async {
        loop {
            let n = sock
                .recv(&mut buf)
                .await
                .map_err(|e| Failed::Silent(format!("{server}: {e}")))?;
            if n >= 2 && u16::from_be_bytes([buf[0], buf[1]]) == id {
                return Ok(buf[..n].to_vec());
            }
        }
    };
    tokio::time::timeout(limit, wait)
        .await
        .map_err(|_| Failed::Silent(format!("{server} did not answer")))?
}

pub async fn tcp(server: SocketAddr, bytes: &[u8], limit: Duration) -> Result<Vec<u8>, String> {
    let len = u16::try_from(bytes.len()).map_err(|_| "the DNS message is too long".to_string())?;
    let work = async {
        let mut s = TcpStream::connect(server)
            .await
            .map_err(|e| format!("{server}: {e}"))?;
        let mut framed = Vec::with_capacity(bytes.len() + 2);
        framed.extend_from_slice(&len.to_be_bytes());
        framed.extend_from_slice(bytes);
        s.write_all(&framed)
            .await
            .map_err(|e| format!("{server}: {e}"))?;
        let mut head = [0u8; 2];
        s.read_exact(&mut head)
            .await
            .map_err(|e| format!("{server} closed the connection: {e}"))?;
        let mut body = vec![0u8; usize::from(u16::from_be_bytes(head))];
        s.read_exact(&mut body)
            .await
            .map_err(|e| format!("{server} closed the connection: {e}"))?;
        Ok::<_, String>(body)
    };
    tokio::time::timeout(limit, work)
        .await
        .map_err(|_| format!("{server} did not answer within {} s", limit.as_secs()))?
}

#[cfg(test)]
mod tests;
