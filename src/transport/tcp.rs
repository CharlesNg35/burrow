use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};
use tokio::net::{TcpListener, TcpSocket, TcpStream};

use crate::config::settings::SocketTuning;

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
