use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream, lookup_host};

pub const LISTEN_BACKLOG: u32 = 4096;

pub fn bind_tcp(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    #[cfg(not(windows))]
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(LISTEN_BACKLOG)
}

use crate::config::settings::SocketTuning;
use crate::proxy::{ProxyConfig, ProxyKind};

pub fn tune(stream: &TcpStream, t: &SocketTuning) {
    let _ = stream.set_nodelay(t.nodelay);
    if let Some(time) = t.keepalive {
        let ka = TcpKeepalive::new().with_time(time);
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "freebsd",
            target_os = "windows"
        ))]
        let ka = ka.with_interval((time / 3).max(Duration::from_secs(1)));
        let _ = SockRef::from(stream).set_tcp_keepalive(&ka);
    }
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

pub async fn connect_any(addrs: &[SocketAddr], timeout: Duration) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no addresses to connect to");
    for addr in addrs {
        match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
            Ok(Ok(s)) => return Ok(s),
            Ok(Err(e)) => last = e,
            Err(_) => {
                last = io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("connecting to {addr} timed out"),
                )
            }
        }
    }
    Err(last)
}

pub async fn dial(
    target: &str,
    proxy: Option<&ProxyConfig>,
    timeout: Duration,
) -> io::Result<TcpStream> {
    match proxy {
        None => connect_any(&resolve(target).await?, timeout).await,
        Some(p) => {
            let mut s = connect_any(&resolve(&p.addr).await?, timeout).await?;
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
