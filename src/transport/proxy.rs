use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::resolve::Resolver;
use super::tcp::connect_any;
use crate::proxy::{ProxyConfig, ProxyKind};

pub async fn dial_tcp(
    target: &str,
    proxy: Option<&ProxyConfig>,
    timeout: Duration,
    resolver: &dyn Resolver,
) -> io::Result<TcpStream> {
    let lookup = |host: &str| {
        let host = host.to_string();
        async move {
            tokio::time::timeout(timeout, resolver.resolve(&host))
                .await
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("looking up {host} timed out"),
                    )
                })?
        }
    };
    match proxy {
        None => connect_any(&lookup(target).await?, timeout).await,
        Some(p) => {
            let mut s = connect_any(&lookup(&p.addr).await?, timeout).await?;
            tokio::time::timeout(timeout, async {
                match p.kind {
                    ProxyKind::Http => http_connect(&mut s, target, p).await,
                    ProxyKind::Socks5 => socks5_connect(&mut s, target, p).await,
                }
            })
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "proxy handshake timed out"))??;
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
    let line = String::from_utf8_lossy(&head);
    let status = line.lines().next().unwrap_or("");
    let code = status.split_whitespace().nth(1).unwrap_or("");
    if code != "200" {
        return Err(proxy_err(format!(
            "proxy refused CONNECT: {}",
            status.trim()
        )));
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
        assert!(
            err.to_string()
                .contains("looking up myserver.example.com:7835")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
