use std::net::{IpAddr, SocketAddr};

pub const MAX_RANGE: u16 = 1000;

pub const SERVER_PORT: u16 = 7835;
pub const WS_PORT: u16 = 7836;
pub const WSS_PORT: u16 = 7837;
pub const ADMIN_PORT: u16 = 7839;
pub const FIRST_SERVICE_PORT: u16 = 7840;

pub fn parse_port(p: &str) -> Result<u16, String> {
    p.trim()
        .parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| format!("\"{}\" is not a valid port (1-65535)", p.trim()))
}

pub fn parse_range(s: &str) -> Result<(u16, u16), String> {
    let s = s.trim();
    let port = |p: &str| {
        p.trim()
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| format!("\"{p}\" is not a valid port (1-65535)"))
    };
    let (first, last) = match s.split_once('-') {
        Some((a, b)) => (port(a)?, port(b)?),
        None => {
            let p = port(s)?;
            (p, p)
        }
    };
    if last < first {
        return Err(format!("the range {s} ends before it starts"));
    }
    let count = last - first + 1;
    if count > MAX_RANGE {
        return Err(format!(
            "the range {s} has {count} ports, the limit is {MAX_RANGE}"
        ));
    }
    Ok((first, count))
}

pub fn show(first: u16, count: u16) -> String {
    match count {
        0 | 1 => first.to_string(),
        n => format!("{first}-{}", first.saturating_add(n - 1)),
    }
}

pub fn show_addr(addr: SocketAddr, count: u16) -> String {
    match addr.ip() {
        IpAddr::V6(ip) => format!("[{ip}]:{}", show(addr.port(), count)),
        IpAddr::V4(ip) => format!("{ip}:{}", show(addr.port(), count)),
    }
}

pub fn parse_ranged_text(
    text: &str,
    default_ip: [u8; 4],
    what: &str,
) -> Result<(SocketAddr, u16), String> {
    let text = text.trim();
    if text.chars().all(|c| c.is_ascii_digit() || c == '-') {
        let (first, count) = parse_range(text)?;
        return Ok((SocketAddr::from((default_ip, first)), count));
    }
    let (host, ports) = match text.rsplit_once(':') {
        Some((h, p)) if !p.contains(']') => (h, p),
        _ => return Err(format!("\"{text}\" is not a valid {what} address")),
    };
    let invalid = || format!("\"{text}\" is not a valid {what} address");
    let ip: IpAddr = match host.strip_prefix('[') {
        Some(inner) => inner
            .strip_suffix(']')
            .and_then(|v6| v6.parse::<std::net::Ipv6Addr>().ok())
            .map(IpAddr::V6)
            .ok_or_else(invalid)?,
        None => host
            .parse::<std::net::Ipv4Addr>()
            .map(IpAddr::V4)
            .map_err(|_| invalid())?,
    };
    let (first, count) = parse_range(ports)?;
    Ok((SocketAddr::new(ip, first), count))
}

pub fn split_target(target: &str) -> Result<(String, u16), String> {
    let Some((host, ports)) = target.rsplit_once(':') else {
        return Ok((target.to_string(), 1));
    };
    if !ports.contains('-') {
        return Ok((target.to_string(), 1));
    }
    let (first, count) = parse_range(ports)?;
    Ok((format!("{host}:{first}"), count))
}

pub fn binds_conflict(a: SocketAddr, b: SocketAddr) -> bool {
    let canonical = |x: SocketAddr| SocketAddr::new(x.ip().to_canonical(), x.port());
    let (a, b) = (canonical(a), canonical(b));
    let covers =
        |x: SocketAddr, y: SocketAddr| x.ip().is_unspecified() && (x.is_ipv6() || y.is_ipv4());
    a.port() == b.port() && (a.ip() == b.ip() || covers(a, b) || covers(b, a))
}

pub fn ranges_conflict(a: SocketAddr, an: u16, b: SocketAddr, bn: u16) -> bool {
    let (a0, a1) = (
        u32::from(a.port()),
        u32::from(a.port()) + u32::from(an.max(1)) - 1,
    );
    let (b0, b1) = (
        u32::from(b.port()),
        u32::from(b.port()) + u32::from(bn.max(1)) - 1,
    );
    let overlap = a0.max(b0);
    a0 <= b1 && b0 <= a1 && {
        let port = u16::try_from(overlap).unwrap_or(u16::MAX);
        binds_conflict(SocketAddr::new(a.ip(), port), SocketAddr::new(b.ip(), port))
    }
}

pub fn port_at(base: &str, index: u16) -> String {
    match base.rsplit_once(':') {
        Some((host, port)) if index > 0 => match port.parse::<u16>() {
            Ok(p) => format!("{host}:{}", p.saturating_add(index)),
            Err(_) => base.to_string(),
        },
        _ => base.to_string(),
    }
}

pub fn sensitive(port: u16) -> Option<&'static str> {
    SENSITIVE_PORTS
        .iter()
        .find(|(p, _)| *p == port)
        .map(|(_, name)| *name)
}

pub const SENSITIVE_PORTS: &[(u16, &str)] = &[
    (22, "SSH"),
    (23, "telnet"),
    (445, "SMB"),
    (2375, "Docker API"),
    (3306, "MySQL"),
    (3389, "RDP"),
    (5432, "PostgreSQL"),
    (5900, "VNC"),
    (6379, "Redis"),
    (9200, "Elasticsearch"),
    (11211, "memcached"),
    (27017, "MongoDB"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn ranges_parse_and_print() {
        assert_eq!(parse_range("30000-30010").unwrap(), (30000, 11));
        assert_eq!(parse_range(" 22 ").unwrap(), (22, 1));
        assert_eq!(parse_range("5-5").unwrap(), (5, 1));
        assert!(
            parse_range("10-9")
                .unwrap_err()
                .contains("ends before it starts")
        );
        assert!(parse_range("0-5").is_err());
        assert!(parse_range("1-2000").unwrap_err().contains("limit is 1000"));
        assert!(parse_range("a-b").is_err());
        assert_eq!(show(30000, 11), "30000-30010");
        assert_eq!(show(22, 1), "22");
        assert_eq!(show_addr(sa("[::1]:80"), 3), "[::1]:80-82");
        assert_eq!(show_addr(sa("0.0.0.0:80"), 1), "0.0.0.0:80");
    }

    #[test]
    fn text_binds_with_ranges() {
        let p = |s: &str| parse_ranged_text(s, [0, 0, 0, 0], "bind");
        assert_eq!(p("30000-30010").unwrap(), (sa("0.0.0.0:30000"), 11));
        assert_eq!(
            p("127.0.0.1:30000-30002").unwrap(),
            (sa("127.0.0.1:30000"), 3)
        );
        assert_eq!(p("[::]:5000-5001").unwrap(), (sa("[::]:5000"), 2));
        assert!(p("::1:22").is_err());
        assert!(p("localhost:22").is_err());
        assert!(p("0.0.0.0:0").is_err());
        for bad in ["[::1:80", "[::1]]:80", "[127.0.0.1]:80", "::1:80"] {
            assert!(p(bad).is_err(), "{bad} was accepted");
        }
        assert_eq!(show(65535, 2), "65535-65535");
        assert_eq!(show(64536, 1000), "64536-65535");
    }

    #[test]
    fn targets_with_ranges() {
        assert_eq!(
            split_target("127.0.0.1:30000-30010").unwrap(),
            ("127.0.0.1:30000".into(), 11)
        );
        assert_eq!(
            split_target("db.local:5432").unwrap(),
            ("db.local:5432".into(), 1)
        );
        assert_eq!(split_target("[::1]:80-81").unwrap(), ("[::1]:80".into(), 2));
        assert!(split_target("h:9-1").is_err());
        assert_eq!(port_at("127.0.0.1:30000", 5), "127.0.0.1:30005");
        assert_eq!(port_at("[::1]:80", 1), "[::1]:81");
        assert_eq!(port_at("db:5432", 0), "db:5432");
    }

    #[test]
    fn overlapping_ranges_conflict() {
        assert!(ranges_conflict(
            sa("0.0.0.0:100"),
            10,
            sa("127.0.0.1:105"),
            1
        ));
        assert!(ranges_conflict(sa("0.0.0.0:100"), 10, sa("0.0.0.0:95"), 6));
        assert!(!ranges_conflict(
            sa("0.0.0.0:100"),
            10,
            sa("0.0.0.0:110"),
            5
        ));
        assert!(!ranges_conflict(
            sa("10.0.0.1:100"),
            10,
            sa("10.0.0.2:100"),
            10
        ));
        assert!(ranges_conflict(sa("0.0.0.0:80"), 1, sa("[::]:80"), 1));
    }

    #[test]
    fn conflicts() {
        let a = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(binds_conflict(a("0.0.0.0:80"), a("127.0.0.1:80")));
        assert!(binds_conflict(a("10.0.0.1:80"), a("10.0.0.1:80")));
        assert!(!binds_conflict(a("10.0.0.1:80"), a("10.0.0.2:80")));
        assert!(!binds_conflict(a("0.0.0.0:80"), a("0.0.0.0:81")));
        assert!(binds_conflict(a("0.0.0.0:80"), a("[::]:80")));
        assert!(binds_conflict(a("0.0.0.0:80"), a("[::ffff:127.0.0.1]:80")));
        assert!(binds_conflict(a("[::ffff:10.0.0.1]:80"), a("10.0.0.1:80")));
        assert!(!binds_conflict(a("[::ffff:10.0.0.1]:80"), a("10.0.0.2:80")));
        assert!(binds_conflict(a("[::]:80"), a("127.0.0.1:80")));
        assert!(binds_conflict(a("[::1]:80"), a("[::]:80")));
        assert!(!binds_conflict(a("0.0.0.0:80"), a("[::1]:80")));
        assert!(!binds_conflict(a("127.0.0.1:80"), a("[::1]:80")));
    }

    #[test]
    fn port_names() {
        assert_eq!(sensitive(5432), Some("PostgreSQL"));
        assert_eq!(sensitive(80), None);
    }
}
