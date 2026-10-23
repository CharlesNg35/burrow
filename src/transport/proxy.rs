use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::resolve::Resolver;
use super::tcp::connect_any;
use crate::proxy::{ProxyConfig, ProxyKind};

#[derive(Debug)]
pub struct LookupTimedOut {
    host: String,
    after: Duration,
}

impl LookupTimedOut {
    pub fn new(host_port: &str, after: Duration) -> LookupTimedOut {
        let host = burrow_syntax::hostport::parse_host_port(host_port)
            .map_or_else(|_| host_port.to_string(), |(host, _)| host);
        LookupTimedOut { host, after }
    }
}

impl std::fmt::Display for LookupTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let after = if self.after.subsec_nanos() == 0 {
            format!("{} s", self.after.as_secs())
        } else {
            format!("{} ms", self.after.as_millis())
        };
        write!(f, "could not look up {} within {after} (DNS)", self.host)
    }
}

impl std::error::Error for LookupTimedOut {}

pub fn lookup_timed_out(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<LookupTimedOut>())
}

pub async fn dial_tcp(
    target: &str,
    proxy: Option<&ProxyConfig>,
    timeout: Duration,
    resolver: &dyn Resolver,
) -> io::Result<TcpStream> {
    dial_tcp_ordered(target, proxy, timeout, resolver, &|_| {}).await
}

pub async fn dial_tcp_ordered(
    target: &str,
    proxy: Option<&ProxyConfig>,
    timeout: Duration,
    resolver: &dyn Resolver,
    order: &(dyn Fn(&mut Vec<std::net::SocketAddr>) + Sync),
) -> io::Result<TcpStream> {
    let lookup = |host: &str| {
        let host = host.to_string();
        async move {
            tokio::time::timeout(timeout, resolver.resolve(&host))
                .await
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, LookupTimedOut::new(&host, timeout))
                })?
        }
    };
    match proxy {
        None => {
            let mut addrs = lookup(target).await?;
            order(&mut addrs);
            connect_any(&addrs, timeout).await
        }
        Some(p) => {
            let mut s = connect_any(&lookup(&p.addr).await?, timeout).await?;
            tokio::time::timeout(timeout, async {
                match p.kind {
                    ProxyKind::Http => http_connect(&mut s, target, p).await,
                    ProxyKind::Socks5 => socks5_connect(&mut s, target, p).await,
                }
            })
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "proxy handshake timed out"))
            .and_then(|r| r)
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionAborted, e.to_string()))?;
            Ok(s)
        }
    }
}

fn proxy_err(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionRefused, msg)
}

async fn http_connect(s: &mut TcpStream, target: &str, p: &ProxyConfig) -> io::Result<()> {
    use base64::Engine as _;
    let mut req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some((u, pw)) = &p.auth {
        let token =
            base64::engine::general_purpose::STANDARD.encode(format!("{u}:{}", pw.as_str()));
        req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await?;
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 8192 {
            return Err(proxy_err("proxy response header is too large".into()));
        }
        if s.read(&mut byte).await? == 0 {
            return Err(proxy_err("proxy closed the connection".into()));
        }
        head.push(byte[0]);
    }
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut reply = httparse::Response::new(&mut headers);
    if !matches!(reply.parse(&head), Ok(httparse::Status::Complete(_))) {
        return Err(proxy_err(
            "proxy answered CONNECT with something that is not a valid HTTP response".into(),
        ));
    }
    if reply.code != Some(200) {
        return Err(proxy_err(
            format!(
                "proxy refused CONNECT: HTTP/1.{} {} {}",
                reply.version.unwrap_or(1),
                reply.code.unwrap_or_default(),
                reply.reason.unwrap_or_default()
            )
            .trim_end()
            .to_string(),
        ));
    }
    Ok(())
}

async fn socks5_connect(s: &mut TcpStream, target: &str, p: &ProxyConfig) -> io::Result<()> {
    let (host, port) = crate::config::check::parse_host_port(target).map_err(proxy_err)?;
    let methods: &[u8] = if p.auth.is_some() {
        &[5, 2, 0, 2]
    } else {
        &[5, 1, 0]
    };
    s.write_all(methods).await?;
    let mut rep = [0u8; 2];
    s.read_exact(&mut rep).await?;
    if rep[0] != 5 {
        return Err(proxy_err("proxy is not a SOCKS5 server".into()));
    }
    match (rep[1], &p.auth) {
        (0, _) => {}
        (2, Some((u, pw))) => {
            let mut m = vec![1, u.len() as u8];
            m.extend_from_slice(u.as_bytes());
            m.push(pw.len() as u8);
            m.extend_from_slice(pw.as_bytes());
            s.write_all(&m).await?;
            s.read_exact(&mut rep).await?;
            if rep[1] != 0 {
                return Err(proxy_err(
                    "SOCKS5 proxy rejected the user name or password".into(),
                ));
            }
        }
        _ if p.auth.is_none() => {
            return Err(proxy_err(
                "SOCKS5 proxy wants a user name and password; give them in the proxy URL, like socks5://user:pass@host:port".into(),
            ));
        }
        _ => {
            return Err(proxy_err(
                "SOCKS5 proxy requires an unsupported authentication method".into(),
            ));
        }
    }
    let mut req = vec![5, 1, 0];
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            req.push(1);
            req.extend_from_slice(&ip.octets());
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            req.push(4);
            req.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            req.push(3);
            req.push(host.len() as u8);
            req.extend_from_slice(host.as_bytes());
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await?;
    if head[1] != 0 {
        return Err(proxy_err(format!(
            "SOCKS5 proxy could not connect (reply code {})",
            head[1]
        )));
    }
    let skip = match head[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut l = [0u8; 1];
            s.read_exact(&mut l).await?;
            l[0] as usize + 2
        }
        _ => return Err(proxy_err("SOCKS5 proxy sent an invalid reply".into())),
    };
    let mut rest = vec![0u8; skip];
    s.read_exact(&mut rest).await?;
    Ok(())
}

#[cfg(test)]
mod lookup_tests {
    use super::*;
    use crate::transport::resolve::Resolved;

    struct Silent;

    impl Resolver for Silent {
        fn resolve<'a>(&'a self, _: &'a str) -> Resolved<'a> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn a_lookup_that_never_answers_times_out() {
        let started = std::time::Instant::now();
        let err = dial_tcp(
            "myserver.example.com:7835",
            None,
            Duration::from_millis(200),
            &Silent,
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            err.to_string(),
            "could not look up myserver.example.com within 200 ms (DNS)"
        );
        assert!(lookup_timed_out(&err));
        assert!(!lookup_timed_out(&io::Error::from(io::ErrorKind::TimedOut)));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

#[cfg(test)]
mod connect_tests {
    use super::*;

    async fn through(reply: &'static [u8]) -> (io::Result<()>, Vec<u8>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && s.read(&mut byte).await.unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let _ = s.write_all(reply).await;
        });
        let mut s = TcpStream::connect(addr).await.unwrap();
        let p = ProxyConfig::parse(&format!("http://{addr}")).unwrap();
        let r = http_connect(&mut s, "target.example:22", &p).await;
        let mut rest = Vec::new();
        if r.is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(2), s.read_to_end(&mut rest)).await;
        }
        (r, rest)
    }

    #[tokio::test]
    async fn a_connect_reply_is_read_up_to_its_head_and_no_further() {
        let (r, rest) =
            through(b"HTTP/1.1 200 Connection established\r\nVia: x\r\n\r\nSSH-2.0-tunnel").await;
        r.unwrap();
        assert_eq!(rest, b"SSH-2.0-tunnel");
        let (r, _) = through(b"HTTP/1.0 200 OK\r\n\r\n").await;
        r.unwrap();
    }

    #[tokio::test]
    async fn a_refused_or_broken_connect_reply_is_named() {
        let err = |r: io::Result<()>| r.unwrap_err().to_string();
        let (r, _) = through(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n").await;
        assert_eq!(
            err(r),
            "proxy refused CONNECT: HTTP/1.1 407 Proxy Authentication Required"
        );
        let (r, _) = through(b"SSH-2.0-OpenSSH\r\n\r\n").await;
        assert!(err(r).contains("not a valid HTTP response"));
        let (r, _) = through(b"HTTP/1.1 200 OK\r\n").await;
        assert!(err(r).contains("proxy closed the connection"));
        let mut big = b"HTTP/1.1 200 OK\r\n".to_vec();
        big.extend(std::iter::repeat_n(b"X-Pad: 0123456789\r\n".as_slice(), 600).flatten());
        big.extend_from_slice(b"\r\n");
        let (r, _) = through(Box::leak(big.into_boxed_slice())).await;
        assert!(err(r).contains("too large"));
    }
}
