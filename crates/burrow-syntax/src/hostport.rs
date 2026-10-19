pub const MAX_TARGET_LEN: usize = 255;

pub fn parse_host_port(s: &str) -> Result<(String, u16), String> {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, tail) = rest
            .split_once(']')
            .ok_or_else(|| format!("\"{s}\" has an unclosed \"[\""))?;
        let p = tail
            .strip_prefix(':')
            .ok_or_else(|| format!("\"{s}\" is missing a port"))?;
        if h.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(format!(
                "only an IPv6 address goes in brackets; write {h}:{p} instead of \"{s}\""
            ));
        }
        (h, p)
    } else {
        match s.rsplit_once(':') {
            Some((h, _)) if h.contains(':') => {
                return Err(format!(
                    "\"{s}\" looks like an IPv6 address; write it in brackets, like [::1]:22"
                ));
            }
            Some((h, p)) => (h, p),
            None => return Err(format!("\"{s}\" is missing a port")),
        }
    };
    if host.is_empty() {
        return Err(format!("\"{s}\" is missing a host"));
    }
    if host.len() > 253 || !host.chars().all(host_char) {
        return Err(format!("\"{host}\" is not a valid host name or IP address"));
    }
    let port: u16 = port
        .parse()
        .map_err(|_| format!("\"{port}\" is not a valid port (1-65535)"))?;
    if port == 0 {
        return Err("port 0 is not allowed".into());
    }
    Ok((host.to_string(), port))
}

fn host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')
}

pub fn check_host(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err("a host name or IP address is required".into());
    }
    if host.len() > 253 || !host.chars().all(host_char) {
        return Err(format!("\"{host}\" is not a valid host name or IP address"));
    }
    if host.contains(':') && host.parse::<std::net::Ipv6Addr>().is_err() {
        return Err(format!("\"{host}\" is not a valid IPv6 address"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port() {
        assert_eq!(parse_host_port("a.b:80").unwrap(), ("a.b".into(), 80));
        assert_eq!(parse_host_port("[::1]:22").unwrap(), ("::1".into(), 22));
        assert!(parse_host_port("::1:22").is_err());
        assert!(parse_host_port("host").is_err());
        assert!(parse_host_port(":80").is_err());
        assert!(parse_host_port("h:0").is_err());
        assert!(parse_host_port("h:99999").is_err());
        assert!(parse_host_port("bad host:1").is_err());
        assert!(parse_host_port("[127.0.0.1]:80").is_err());
        assert!(parse_host_port("[localhost]:80").is_err());
    }

    #[test]
    fn hosts() {
        assert!(check_host("db.local").is_ok());
        assert!(check_host("::1").is_ok());
        assert!(check_host("a b").is_err());
        assert!(check_host("1:2").is_err());
        assert!(check_host("").is_err());
    }
}
