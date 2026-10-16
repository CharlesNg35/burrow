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

pub const ATTEMPT_DELAY: Duration = Duration::from_millis(250);

pub fn interleave(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    let first_v6 = addrs.first().is_some_and(SocketAddr::is_ipv6);
    let (mut a, mut b): (Vec<SocketAddr>, Vec<SocketAddr>) =
        addrs.iter().partition(|x| x.is_ipv6() == first_v6);
    a.reverse();
    b.reverse();
    let mut out = Vec::with_capacity(addrs.len());
    while !a.is_empty() || !b.is_empty() {
        out.extend(a.pop());
        out.extend(b.pop());
    }
    out
}

pub async fn connect_any(addrs: &[SocketAddr], timeout: Duration) -> io::Result<TcpStream> {
    let ordered = interleave(addrs);
    let race = async {
        let mut attempts = tokio::task::JoinSet::new();
        let mut next = ordered.iter().copied();
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no addresses to connect to");
        if let Some(a) = next.next() {
            attempts.spawn(async move { (a, TcpStream::connect(a).await) });
        }
        loop {
            let more = next.len() > 0;
            tokio::select! {
                done = attempts.join_next(), if !attempts.is_empty() => match done {
                    Some(Ok((_, Ok(s)))) => return Ok(s),
                    Some(Ok((_, Err(e)))) => {
                        last = e;
                        if let Some(a) = next.next() {
                            attempts.spawn(async move { (a, TcpStream::connect(a).await) });
                        }
                    }
                    _ => {}
                },
                _ = tokio::time::sleep(ATTEMPT_DELAY), if more => {
                    if let Some(a) = next.next() {
                        attempts.spawn(async move { (a, TcpStream::connect(a).await) });
                    }
                }
            }
            if attempts.is_empty() && next.len() == 0 {
                return Err(last);
            }
        }
    };
    tokio::time::timeout(timeout, race)
        .await
        .unwrap_or_else(|_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "connection attempts timed out",
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn interleaves_families_starting_with_the_first() {
        let input = [
            a("[::1]:1"),
            a("[::2]:1"),
            a("1.1.1.1:1"),
            a("[::3]:1"),
            a("2.2.2.2:1"),
        ];
        let out = interleave(&input);
        assert_eq!(
            out,
            vec![
                a("[::1]:1"),
                a("1.1.1.1:1"),
                a("[::2]:1"),
                a("2.2.2.2:1"),
                a("[::3]:1")
            ]
        );
        assert_eq!(interleave(&[a("1.1.1.1:1")]), vec![a("1.1.1.1:1")]);
    }

    #[tokio::test]
    async fn black_hole_address_does_not_block_a_good_one() {
        let good = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let good_addr = good.local_addr().unwrap();
        let black_hole = a("10.255.255.1:9");
        let start = std::time::Instant::now();
        let s = connect_any(&[black_hole, good_addr], Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(s.peer_addr().unwrap(), good_addr);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "took {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn refused_addresses_fail_fast() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let start = std::time::Instant::now();
        assert!(
            connect_any(&[addr, addr], Duration::from_secs(5))
                .await
                .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
