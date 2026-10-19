use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub fn parse_ip(text: &str) -> Result<IpAddr, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("an IP address is required".into());
    }
    if let Some(inner) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return match inner.parse::<Ipv6Addr>() {
            Ok(_) => Err("leave out the brackets here".into()),
            Err(_) => Err(format!("\"{t}\" is not an IP address")),
        };
    }
    if t.contains(':') {
        return t
            .parse::<Ipv6Addr>()
            .map(normalize6)
            .map_err(|_| format!("\"{t}\" is not an IPv6 address"));
    }
    let parts: Vec<&str> = t.split('.').collect();
    if parts.len() != 4 {
        return Err(if t.chars().all(|c| c.is_ascii_digit() || c == '.') {
            format!("\"{t}\" is not an IPv4 address: it needs 4 numbers separated by dots")
        } else {
            format!("\"{t}\" is not an IP address")
        });
    }
    let mut octets = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("\"{t}\" is not an IPv4 address"));
        }
        if p.len() > 1 && p.starts_with('0') {
            return Err(format!(
                "{t} is ambiguous: leave out the leading zero in {p}"
            ));
        }
        let n: u32 = p
            .parse()
            .map_err(|_| format!("\"{t}\" is not an IPv4 address"))?;
        if n > 255 {
            return Err(format!("{t} is not an IPv4 address: {n} is above 255"));
        }
        octets[i] = n as u8;
    }
    Ok(IpAddr::V4(Ipv4Addr::from(octets)))
}

fn normalize6(ip: Ipv6Addr) -> IpAddr {
    ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4)
}

pub fn kind(ip: IpAddr) -> &'static str {
    match ip {
        IpAddr::V4(v4) if v4.is_unspecified() => "all IPv4 addresses",
        IpAddr::V6(v6) if v6.is_unspecified() => "all IPv6 addresses",
        ip if ip.is_loopback() => "this machine only",
        IpAddr::V4(v4) if v4.is_private() => "private",
        IpAddr::V4(v4) if v4.is_link_local() => "link-local",
        IpAddr::V6(v6) if (v6.segments()[0] & 0xfe00) == 0xfc00 => "private",
        IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80 => "link-local",
        _ => "public",
    }
}

pub fn show_with_port(ip: IpAddr, port: &str) -> String {
    match ip {
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
        IpAddr::V4(v4) => format!("{v4}:{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_errors() {
        assert_eq!(parse_ip("10.0.0.1").unwrap().to_string(), "10.0.0.1");
        assert!(
            parse_ip("300.1.1.1")
                .unwrap_err()
                .contains("300 is above 255")
        );
        assert!(parse_ip("010.0.0.1").unwrap_err().contains("ambiguous"));
        assert!(parse_ip("[::1]").unwrap_err().contains("brackets"));
        assert_eq!(parse_ip("::ffff:1.2.3.4").unwrap().to_string(), "1.2.3.4");
        assert_eq!(parse_ip("::").unwrap().to_string(), "::");
        assert!(parse_ip("1.2.3").unwrap_err().contains("4 numbers"));
        assert!(parse_ip("host").is_err());
    }

    #[test]
    fn kinds() {
        assert_eq!(kind("0.0.0.0".parse().unwrap()), "all IPv4 addresses");
        assert_eq!(kind("127.0.0.1".parse().unwrap()), "this machine only");
        assert_eq!(kind("10.0.0.5".parse().unwrap()), "private");
        assert_eq!(kind("203.0.113.5".parse().unwrap()), "public");
        assert_eq!(show_with_port("::1".parse().unwrap(), "22"), "[::1]:22");
    }
}
