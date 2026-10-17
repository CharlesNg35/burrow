use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::protocol::Status;

const VERSION: u8 = 5;
const NO_AUTH: u8 = 0;
const NO_METHOD: u8 = 0xff;
const CONNECT: u8 = 1;

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

pub async fn request<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S) -> io::Result<String> {
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await?;
    if head[0] != VERSION {
        return Err(bad("not a SOCKS5 client"));
    }
    let mut methods = vec![0u8; head[1] as usize];
    s.read_exact(&mut methods).await?;
    if !methods.contains(&NO_AUTH) {
        s.write_all(&[VERSION, NO_METHOD]).await?;
        return Err(bad("the SOCKS client wants authentication"));
    }
    s.write_all(&[VERSION, NO_AUTH]).await?;
    let mut req = [0u8; 4];
    s.read_exact(&mut req).await?;
    if req[0] != VERSION {
        return Err(bad("not a SOCKS5 request"));
    }
    if req[1] != CONNECT {
        reply(s, Reply::CommandNotSupported).await?;
        return Err(bad("only CONNECT is supported"));
    }
    let host = match req[3] {
        1 => {
            let mut ip = [0u8; 4];
            s.read_exact(&mut ip).await?;
            Ipv4Addr::from(ip).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            s.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| bad("the host name is not text"))?
        }
        4 => {
            let mut ip = [0u8; 16];
            s.read_exact(&mut ip).await?;
            format!("[{}]", Ipv6Addr::from(ip))
        }
        _ => {
            reply(s, Reply::AddressNotSupported).await?;
            return Err(bad("unknown address type"));
        }
    };
    let mut port = [0u8; 2];
    s.read_exact(&mut port).await?;
    Ok(format!("{host}:{}", u16::from_be_bytes(port)))
}

pub async fn reply<S: AsyncWrite + Unpin>(s: &mut S, r: Reply) -> io::Result<()> {
    s.write_all(&[VERSION, r as u8, 0, 1, 0, 0, 0, 0, 0, 0])
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ask(bytes: &[u8]) -> (io::Result<String>, Vec<u8>) {
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
        let (got, answer) = ask(&[5, 1, 0, 5, 1, 0, 1, 10, 0, 0, 5, 0, 22]).await;
        assert_eq!(got.unwrap(), "10.0.0.5:22");
        assert_eq!(answer, [5, 0]);
        let mut name = vec![5, 2, 2, 0, 5, 1, 0, 3, 9];
        name.extend_from_slice(b"nas.local");
        name.extend_from_slice(&5000u16.to_be_bytes());
        assert_eq!(ask(&name).await.0.unwrap(), "nas.local:5000");
        let mut v6 = vec![5, 1, 0, 5, 1, 0, 4];
        v6.extend_from_slice(&Ipv6Addr::LOCALHOST.octets());
        v6.extend_from_slice(&[0, 80]);
        assert_eq!(ask(&v6).await.0.unwrap(), "[::1]:80");
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

    #[test]
    fn refusals_map_to_socks_replies() {
        assert_eq!(Reply::from(Status::Denied), Reply::NotAllowed);
        assert_eq!(Reply::from(Status::Unreachable), Reply::Refused);
        assert_eq!(Reply::from(Status::Offline), Reply::NetworkUnreachable);
    }
}
