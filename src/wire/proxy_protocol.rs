use std::net::{IpAddr, SocketAddr};

use burrow_config::ProxyVersion;

pub const V2_SIGNATURE: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

fn same_family(src: SocketAddr, dst: SocketAddr) -> (IpAddr, IpAddr) {
    match (canonical(src.ip()), canonical(dst.ip())) {
        (IpAddr::V4(s), IpAddr::V4(d)) => (IpAddr::V4(s), IpAddr::V4(d)),
        (s, d) => (IpAddr::V6(v6(s)), IpAddr::V6(v6(d))),
    }
}

fn v6(ip: IpAddr) -> std::net::Ipv6Addr {
    match ip {
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
        IpAddr::V6(v6) => v6,
    }
}

pub fn header(version: ProxyVersion, src: SocketAddr, dst: SocketAddr) -> Vec<u8> {
    let (s, d) = same_family(src, dst);
    match version {
        ProxyVersion::V1 => {
            let family = if s.is_ipv4() { "TCP4" } else { "TCP6" };
            format!("PROXY {family} {s} {d} {} {}\r\n", src.port(), dst.port()).into_bytes()
        }
        ProxyVersion::V2 => {
            let mut out = V2_SIGNATURE.to_vec();
            out.push(0x21);
            match (s, d) {
                (IpAddr::V4(s), IpAddr::V4(d)) => {
                    out.push(0x11);
                    out.extend_from_slice(&12u16.to_be_bytes());
                    out.extend_from_slice(&s.octets());
                    out.extend_from_slice(&d.octets());
                }
                (s, d) => {
                    out.push(0x21);
                    out.extend_from_slice(&36u16.to_be_bytes());
                    out.extend_from_slice(&v6(s).octets());
                    out.extend_from_slice(&v6(d).octets());
                }
            }
            out.extend_from_slice(&src.port().to_be_bytes());
            out.extend_from_slice(&dst.port().to_be_bytes());
            out
        }
    }
}

pub fn local(version: ProxyVersion) -> Vec<u8> {
    match version {
        ProxyVersion::V1 => b"PROXY UNKNOWN\r\n".to_vec(),
        ProxyVersion::V2 => {
            let mut out = V2_SIGNATURE.to_vec();
            out.extend_from_slice(&[0x20, 0x00, 0, 0]);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn v1_text() {
        assert_eq!(
            header(ProxyVersion::V1, sa("81.2.3.4:51234"), sa("10.0.0.5:443")),
            b"PROXY TCP4 81.2.3.4 10.0.0.5 51234 443\r\n"
        );
        assert_eq!(
            header(ProxyVersion::V1, sa("[2001:db8::1]:5000"), sa("[::1]:22")),
            b"PROXY TCP6 2001:db8::1 ::1 5000 22\r\n"
        );
        assert_eq!(
            header(
                ProxyVersion::V1,
                sa("[::ffff:81.2.3.4]:1"),
                sa("10.0.0.5:2")
            ),
            b"PROXY TCP4 81.2.3.4 10.0.0.5 1 2\r\n",
            "IPv4-mapped addresses are sent as IPv4"
        );
        assert_eq!(
            header(ProxyVersion::V1, sa("81.2.3.4:1"), sa("[2001:db8::5]:2")),
            b"PROXY TCP6 ::ffff:81.2.3.4 2001:db8::5 1 2\r\n",
            "mixed families are both sent as IPv6"
        );
    }

    #[test]
    fn local_headers_carry_no_address() {
        assert_eq!(local(ProxyVersion::V1), b"PROXY UNKNOWN\r\n");
        let v2 = local(ProxyVersion::V2);
        assert_eq!(&v2[..12], &V2_SIGNATURE);
        assert_eq!(&v2[12..], &[0x20, 0x00, 0, 0]);
    }

    #[test]
    fn v2_binary() {
        let h = header(ProxyVersion::V2, sa("81.2.3.4:51234"), sa("10.0.0.5:443"));
        let mut want = V2_SIGNATURE.to_vec();
        want.extend_from_slice(&[0x21, 0x11, 0, 12, 81, 2, 3, 4, 10, 0, 0, 5]);
        want.extend_from_slice(&51234u16.to_be_bytes());
        want.extend_from_slice(&443u16.to_be_bytes());
        assert_eq!(h, want);
        let h = header(ProxyVersion::V2, sa("[2001:db8::1]:5000"), sa("[::1]:22"));
        assert_eq!(h.len(), 16 + 36);
        assert_eq!(&h[12..16], &[0x21, 0x21, 0, 36]);
        assert_eq!(
            &h[16..32],
            &"2001:db8::1"
                .parse::<std::net::Ipv6Addr>()
                .unwrap()
                .octets()
        );
        assert_eq!(&h[48..52], &[0x13, 0x88, 0, 22]);
    }
}
