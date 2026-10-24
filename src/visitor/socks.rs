use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::wire::protocol::Status;

const VERSION: u8 = 5;
const NO_AUTH: u8 = 0;
const NO_METHOD: u8 = 0xff;
const CONNECT: u8 = 1;
const UDP_ASSOCIATE: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Succeeded = 0,
    Failure = 1,
    NotAllowed = 2,
    NetworkUnreachable = 3,
    HostUnreachable = 4,
    Refused = 5,
    CommandNotSupported = 7,
    AddressNotSupported = 8,
}

impl Reply {
    pub fn meaning(self) -> &'static str {
        match self {
            Reply::Succeeded => "succeeded",
            Reply::Failure => "general failure",
            Reply::NotAllowed => "connection not allowed by ruleset",
            Reply::NetworkUnreachable => "network unreachable",
            Reply::HostUnreachable => "host unreachable",
            Reply::Refused => "connection refused",
            Reply::CommandNotSupported => "command not supported",
            Reply::AddressNotSupported => "address type not supported",
        }
    }
}

impl From<Status> for Reply {
    fn from(s: Status) -> Self {
        match s {
            Status::Ok => Reply::Succeeded,
            Status::Denied | Status::Forbidden => Reply::NotAllowed,
            Status::Offline => Reply::NetworkUnreachable,
            Status::Unresolved => Reply::HostUnreachable,
            Status::Unreachable => Reply::Refused,
            Status::Invalid | Status::Busy => Reply::Failure,
        }
    }
}

fn bad(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}

fn unsupported(why: String) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, why)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    Connect(String),
    Associate(Hint),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hint {
    pub ip: Option<IpAddr>,
    pub port: u16,
}

enum Addr {
    Ip(IpAddr),
    Name(String),
}

pub async fn request<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S) -> io::Result<Asked> {
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await?;
    if head[0] != VERSION {
        return Err(bad(
            "not a SOCKS5 client (SOCKS4 and HTTP proxy requests are not supported)",
        ));
    }
    let mut methods = vec![0u8; head[1] as usize];
    s.read_exact(&mut methods).await?;
    if !methods.contains(&NO_AUTH) {
        s.write_all(&[VERSION, NO_METHOD]).await?;
        return Err(unsupported(
            "the app asks for a user name and password, but this SOCKS5 proxy takes none, so it was told: no acceptable method; set the app to no authentication".into(),
        ));
    }
    s.write_all(&[VERSION, NO_AUTH]).await?;
    let mut req = [0u8; 4];
    s.read_exact(&mut req).await?;
    if req[0] != VERSION {
        return Err(bad("not a SOCKS5 request"));
    }
    if req[1] != CONNECT && req[1] != UDP_ASSOCIATE {
        reply(s, Reply::CommandNotSupported).await?;
        let asked = match req[1] {
            2 => "BIND".to_string(),
            n => format!("command {n}"),
        };
        return Err(unsupported(format!(
            "the app asked for {asked}, but only CONNECT and UDP ASSOCIATE are supported, so it was told: {}",
            Reply::CommandNotSupported.meaning()
        )));
    }
    let addr = match req[3] {
        1 => {
            let mut ip = [0u8; 4];
            s.read_exact(&mut ip).await?;
            Addr::Ip(Ipv4Addr::from(ip).into())
        }
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            s.read_exact(&mut name).await?;
            let name = String::from_utf8(name).map_err(|_| bad("the host name is not text"))?;
            match name.parse::<Ipv6Addr>() {
                Ok(ip) => Addr::Ip(ip.into()),
                Err(_) => Addr::Name(name),
            }
        }
        4 => {
            let mut ip = [0u8; 16];
            s.read_exact(&mut ip).await?;
            Addr::Ip(Ipv6Addr::from(ip).into())
        }
        n => {
            reply(s, Reply::AddressNotSupported).await?;
            return Err(unsupported(format!(
                "the app sent address type {n}, which SOCKS5 does not define, so it was told: {}",
                Reply::AddressNotSupported.meaning()
            )));
        }
    };
    let mut port = [0u8; 2];
    s.read_exact(&mut port).await?;
    let port = u16::from_be_bytes(port);
    if req[1] == UDP_ASSOCIATE {
        let ip = match addr {
            Addr::Ip(ip) if !ip.is_unspecified() => Some(ip),
            _ => None,
        };
        return Ok(Asked::Associate(Hint { ip, port }));
    }
    let host = match addr {
        Addr::Ip(IpAddr::V4(ip)) => ip.to_string(),
        Addr::Ip(IpAddr::V6(ip)) => format!("[{ip}]"),
        Addr::Name(name) => name,
    };
    Ok(Asked::Connect(format!("{host}:{port}")))
}

pub async fn reply_bound<S: AsyncWrite + Unpin>(
    s: &mut S,
    r: Reply,
    at: SocketAddr,
) -> io::Result<()> {
    let mut out = vec![VERSION, r as u8, 0];
    match at.ip() {
        IpAddr::V4(ip) => {
            out.push(1);
            out.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            out.push(4);
            out.extend_from_slice(&ip.octets());
        }
    }
    out.extend_from_slice(&at.port().to_be_bytes());
    s.write_all(&out).await
}

pub async fn reply<S: AsyncWrite + Unpin>(s: &mut S, r: Reply) -> io::Result<()> {
    s.write_all(&[VERSION, r as u8, 0, 1, 0, 0, 0, 0, 0, 0])
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ask(bytes: &[u8]) -> (io::Result<Asked>, Vec<u8>) {
        let (mut client, mut server) = tokio::io::duplex(1024);
        client.write_all(bytes).await.unwrap();
        let got = request(&mut server).await;
        drop(server);
        let mut answer = Vec::new();
        client.read_to_end(&mut answer).await.unwrap();
        (got, answer)
    }

    #[tokio::test]
    async fn connect_requests_name_the_target() {
        let connect = |t: &str| Asked::Connect(t.to_string());
        let (got, answer) = ask(&[5, 1, 0, 5, 1, 0, 1, 10, 0, 0, 5, 0, 22]).await;
        assert_eq!(got.unwrap(), connect("10.0.0.5:22"));
        assert_eq!(answer, [5, 0]);
        let mut name = vec![5, 2, 2, 0, 5, 1, 0, 3, 9];
        name.extend_from_slice(b"nas.local");
        name.extend_from_slice(&5000u16.to_be_bytes());
        assert_eq!(ask(&name).await.0.unwrap(), connect("nas.local:5000"));
        let mut v6 = vec![5, 1, 0, 5, 1, 0, 4];
        v6.extend_from_slice(&Ipv6Addr::LOCALHOST.octets());
        v6.extend_from_slice(&[0, 80]);
        assert_eq!(ask(&v6).await.0.unwrap(), connect("[::1]:80"));
        let mut v6_name = vec![5, 1, 0, 5, 1, 0, 3, 3];
        v6_name.extend_from_slice(b"::1");
        v6_name.extend_from_slice(&[0, 80]);
        assert_eq!(ask(&v6_name).await.0.unwrap(), connect("[::1]:80"));
    }

    #[tokio::test]
    async fn other_requests_are_refused() {
        let (got, answer) = ask(&[5, 1, 2]).await;
        assert!(got.is_err());
        assert_eq!(answer, [5, 0xff], "no acceptable method");
        let (got, answer) = ask(&[5, 1, 0, 5, 2, 0, 1, 1, 2, 3, 4, 0, 1]).await;
        assert!(got.is_err());
        assert_eq!(answer[..4], [5, 0, 5, 7], "BIND is not supported");
        assert!(
            ask(&[4, 1, 0, 80]).await.0.is_err(),
            "SOCKS4 is not supported"
        );
    }

    #[tokio::test]
    async fn udp_associate_requests_name_the_address_the_app_sends_from() {
        let (got, answer) = ask(&[5, 1, 0, 5, 3, 0, 1, 0, 0, 0, 0, 0, 0]).await;
        assert_eq!(answer, [5, 0], "the association itself answers");
        assert_eq!(got.unwrap(), Asked::Associate(Hint { ip: None, port: 0 }));
        let (got, _) = ask(&[5, 1, 0, 5, 3, 0, 1, 127, 0, 0, 1, 0x13, 0x88]).await;
        assert_eq!(
            got.unwrap(),
            Asked::Associate(Hint {
                ip: Some(Ipv4Addr::LOCALHOST.into()),
                port: 5000
            })
        );
        let mut named = vec![5, 1, 0, 5, 3, 0, 3, 4];
        named.extend_from_slice(b"host");
        named.extend_from_slice(&[0, 7]);
        assert_eq!(
            ask(&named).await.0.unwrap(),
            Asked::Associate(Hint { ip: None, port: 7 })
        );
    }

    #[tokio::test]
    async fn bound_replies_carry_the_relay_address() {
        let mut out = Vec::new();
        reply_bound(
            &mut out,
            Reply::Succeeded,
            "127.0.0.1:5000".parse().unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out, [5, 0, 0, 1, 127, 0, 0, 1, 0x13, 0x88]);
        let mut out = Vec::new();
        reply_bound(&mut out, Reply::Succeeded, "[::1]:53".parse().unwrap())
            .await
            .unwrap();
        let mut want = vec![5, 0, 0, 4];
        want.extend_from_slice(&Ipv6Addr::LOCALHOST.octets());
        want.extend_from_slice(&[0, 53]);
        assert_eq!(out, want);
    }

    #[tokio::test]
    async fn authentication_requests_are_refused_with_the_reason() {
        let (got, answer) = ask(&[5, 1, 2]).await;
        assert_eq!(answer, [5, 0xff]);
        let e = got.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        assert!(e.to_string().contains("user name and password"), "{e}");
        let (got, _) = ask(&[5, 1, 0, 5, 2, 0, 1, 1, 2, 3, 4, 0, 1]).await;
        assert!(got.unwrap_err().to_string().contains("BIND"));
        let (got, _) = ask(&[4, 1, 0, 80]).await;
        assert_eq!(got.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn replies_have_their_standard_names() {
        assert_eq!(
            Reply::NotAllowed.meaning(),
            "connection not allowed by ruleset"
        );
        assert_eq!(
            Reply::CommandNotSupported.meaning(),
            "command not supported"
        );
        assert_eq!(Reply::Refused.meaning(), "connection refused");
    }

    #[test]
    fn refusals_map_to_socks_replies() {
        assert_eq!(Reply::from(Status::Denied), Reply::NotAllowed);
        assert_eq!(Reply::from(Status::Unreachable), Reply::Refused);
        assert_eq!(Reply::from(Status::Offline), Reply::NetworkUnreachable);
    }
}
