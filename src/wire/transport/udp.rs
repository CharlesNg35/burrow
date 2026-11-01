use std::net::SocketAddr;

use tokio::net::UdpSocket;

const ROOM: usize = 64 * 1024;
const SHARED_RECEIVE: usize = 7 << 20;
const PEER_RECEIVE: usize = 256 * 1024;

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
    bind_with(addr, grow_shared)
}

pub async fn bind_udp_peer(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    bind_udp_peer_now(addr)
}

pub fn bind_udp_peer_now(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    bind_with(addr, |s| grow_receive(s, PEER_RECEIVE))
}

fn bind_with(
    addr: SocketAddr,
    grow: impl FnOnce(&socket2::SockRef<'_>),
) -> std::io::Result<UdpSocket> {
    let sock = std_socket(addr, socket2::Type::DGRAM)?;
    sock.set_nonblocking(true)?;
    let sock = UdpSocket::from_std(sock.into())?;
    let s = socket2::SockRef::from(&sock);
    if s.send_buffer_size().is_ok_and(|n| n < ROOM) {
        let _ = s.set_send_buffer_size(ROOM);
    }
    grow(&s);
    Ok(sock)
}

fn grow_receive(s: &socket2::SockRef<'_>, want: usize) {
    let mut ask = want;
    while ask >= 2 * ROOM && s.recv_buffer_size().is_ok_and(|n| n < want) {
        if s.set_recv_buffer_size(ask).is_ok() {
            return;
        }
        ask /= 2;
    }
}

fn grow_shared(s: &socket2::SockRef<'_>) {
    let granted = settle(s, SHARED_RECEIVE, force_receive);
    if cfg!(any(target_os = "linux", target_os = "android")) {
        static SHORT: std::sync::Once = std::sync::Once::new();
        note_short(&SHORT, SHARED_RECEIVE, granted, warn_short);
    }
}

fn settle(
    s: &socket2::SockRef<'_>,
    want: usize,
    force: fn(&socket2::SockRef<'_>, usize) -> std::io::Result<()>,
) -> usize {
    if force(s, want).is_err() {
        grow_receive(s, want);
    }
    s.recv_buffer_size().unwrap_or(0)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn force_receive(s: &socket2::SockRef<'_>, want: usize) -> std::io::Result<()> {
    rustix::net::sockopt::set_socket_recv_buffer_size_force(&**s, want).map_err(Into::into)
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn force_receive(_: &socket2::SockRef<'_>, _: usize) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

fn short_of(asked: usize, granted: usize) -> bool {
    granted < asked / 2
}

fn note_short(once: &std::sync::Once, asked: usize, granted: usize, warn: fn(usize, usize)) {
    if short_of(asked, granted) {
        once.call_once(|| warn(asked, granted));
    }
}

fn warn_short(asked: usize, granted: usize) {
    tracing::warn!(
        "the system gave a UDP port {} KiB of receive buffer, well below the {} KiB burrow asked for, so bursts of datagrams may be dropped; running burrow as root (CAP_NET_ADMIN) or raising net.core.rmem_max lifts the cap, see https://github.com/charlesng35/burrow/blob/main/docs/services.md#expose-a-udp-service",
        granted / 1024,
        asked / 1024
    );
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

    #[tokio::test]
    async fn a_shared_socket_asks_for_more_receive_room_than_a_peer_socket() {
        let shared = bind_udp("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let peer = bind_udp_peer("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let room = |u: &UdpSocket| socket2::SockRef::from(u).recv_buffer_size().unwrap();
        assert!(room(&shared) >= 2 * ROOM);
        assert!(room(&peer) >= 2 * ROOM);
        assert!(room(&shared) >= room(&peer));
    }

    fn socket() -> UdpSocket {
        let s = std_socket("127.0.0.1:0".parse().unwrap(), socket2::Type::DGRAM).unwrap();
        s.set_nonblocking(true).unwrap();
        UdpSocket::from_std(s.into()).unwrap()
    }

    #[test]
    fn well_below_means_under_half_of_the_ask() {
        assert!(short_of(SHARED_RECEIVE, 425_984));
        assert!(short_of(SHARED_RECEIVE, SHARED_RECEIVE / 2 - 1));
        assert!(!short_of(SHARED_RECEIVE, SHARED_RECEIVE / 2));
        assert!(!short_of(SHARED_RECEIVE, 4 << 20));
        assert!(!short_of(SHARED_RECEIVE, 2 * SHARED_RECEIVE));
        assert!(!short_of(PEER_RECEIVE, 425_984));
    }

    #[test]
    fn a_short_buffer_is_reported_once_per_process() {
        static SEEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        fn count(_: usize, _: usize) {
            SEEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let once = std::sync::Once::new();
        note_short(&once, SHARED_RECEIVE, 14 << 20, count);
        assert_eq!(SEEN.load(std::sync::atomic::Ordering::SeqCst), 0);
        note_short(&once, SHARED_RECEIVE, 425_984, count);
        note_short(&once, SHARED_RECEIVE, 425_984, count);
        note_short(&once, SHARED_RECEIVE, 200_000, count);
        assert_eq!(SEEN.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_refused_force_falls_back_to_asking_as_before() {
        fn refused(_: &socket2::SockRef<'_>, _: usize) -> std::io::Result<()> {
            Err(std::io::ErrorKind::PermissionDenied.into())
        }
        let forced = socket();
        let plain = socket();
        let s = socket2::SockRef::from(&plain);
        grow_receive(&s, SHARED_RECEIVE);
        let granted = settle(&socket2::SockRef::from(&forced), SHARED_RECEIVE, refused);
        assert_eq!(granted, s.recv_buffer_size().unwrap());
        assert!(granted >= 2 * ROOM);
    }

    #[tokio::test]
    async fn a_force_that_succeeds_skips_the_fallback() {
        fn small(s: &socket2::SockRef<'_>, _: usize) -> std::io::Result<()> {
            s.set_recv_buffer_size(ROOM)
        }
        let sock = socket();
        let s = socket2::SockRef::from(&sock);
        let before = {
            s.set_recv_buffer_size(ROOM).unwrap();
            s.recv_buffer_size().unwrap()
        };
        assert_eq!(settle(&s, SHARED_RECEIVE, small), before);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn the_real_force_either_grants_the_whole_ask_or_is_refused() {
        let sock = socket();
        let s = socket2::SockRef::from(&sock);
        match force_receive(&s, SHARED_RECEIVE) {
            Ok(()) => assert!(s.recv_buffer_size().unwrap() >= SHARED_RECEIVE),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}"),
        }
    }
}
