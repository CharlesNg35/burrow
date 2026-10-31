use std::net::SocketAddr;

use tokio::net::UdpSocket;

const ROOM: usize = 64 * 1024;

thread_local! {
    static RECEIVE: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0u8; ROOM]);
}

pub fn try_recv_owned(sock: &UdpSocket) -> std::io::Result<(Vec<u8>, SocketAddr)> {
    RECEIVE.with(|buf| {
        let mut buf = buf.borrow_mut();
        let (n, from) = sock.try_recv_from(&mut buf)?;
        Ok((buf[..n].to_vec(), from))
    })
}

pub fn std_socket(addr: SocketAddr, kind: socket2::Type) -> std::io::Result<socket2::Socket> {
    let domain = if addr.is_ipv4() {
        socket2::Domain::IPV4
    } else {
        socket2::Domain::IPV6
    };
    if kind == socket2::Type::STREAM {
        super::tcp::shadowed(addr)?;
    }
    let sock = socket2::Socket::new(domain, kind, None)?;
    if cfg!(not(windows)) && kind == socket2::Type::STREAM {
        sock.set_reuse_address(true)?;
    }
    if addr.is_ipv6() && addr.ip().is_unspecified() {
        sock.set_only_v6(false)?;
    }
    sock.bind(&addr.into())?;
    Ok(sock)
}

pub async fn bind_udp(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    bind_udp_now(addr)
}

pub fn bind_udp_now(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let sock = std_socket(addr, socket2::Type::DGRAM)?;
    sock.set_nonblocking(true)?;
    let sock = UdpSocket::from_std(sock.into())?;
    let s = socket2::SockRef::from(&sock);
    if s.send_buffer_size().is_ok_and(|n| n < ROOM) {
        let _ = s.set_send_buffer_size(ROOM);
    }
    if s.recv_buffer_size().is_ok_and(|n| n < 2 * ROOM) {
        let _ = s.set_recv_buffer_size(2 * ROOM);
    }
    Ok(sock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_largest_ipv4_datagram_gets_through() {
        let a = bind_udp("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let b = bind_udp("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let big = vec![7u8; 65507];
        a.send_to(&big, b.local_addr().unwrap()).await.unwrap();
        let mut buf = vec![0u8; 65536];
        let (n, _) = b.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], &big[..]);
    }
}
