use std::fmt;

use crate::hostport::parse_host_port;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Tcp {
        host_port: String,
    },
    WebSocket {
        tls: bool,
        host: String,
        port: u16,
        path: String,
    },
}

impl Endpoint {
    pub fn parse(s: &str) -> Result<Endpoint, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("the address is empty".into());
        }
        let Some((scheme, rest)) = s.split_once("://") else {
            if s.contains('@') {
                return Err(
                    "user names and passwords are not allowed in the server address".into(),
                );
            }
            let host_port = with_default_port(s, crate::ports::SERVER_PORT);
            parse_host_port(&host_port)?;
            return Ok(Endpoint::Tcp { host_port });
        };
        let tls = match scheme.to_ascii_lowercase().as_str() {
            "wss" => true,
            "ws" => false,
            other => {
                return Err(format!(
                    "\"{other}://\" is not supported; use host:port for TCP, or ws:// or wss:// for WebSocket"
                ));
            }
        };
        if rest.contains('@') {
            return Err("user names and passwords are not allowed in the server URL".into());
        }
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if !valid_path(path) {
            return Err(format!(
                "path \"{path}\" must use letters, digits and - . _ ~ / only"
            ));
        }
        let default_port = if tls { 443 } else { 80 };
        let with_port = if has_port(authority) {
            authority.to_string()
        } else if authority.ends_with(':') {
            return Err(format!("\"{authority}\" is missing the port after \":\""));
        } else if authority.contains(':') && !authority.starts_with('[') {
            return Err(format!(
                "\"{authority}\" looks like an IPv6 address; write it in brackets, like [2001:db8::1]"
            ));
        } else {
            format!("{authority}:{default_port}")
        };
        let (host, port) = parse_host_port(&with_port)?;
        Ok(Endpoint::WebSocket {
            tls,
            host,
            port,
            path: path.to_string(),
        })
    }

    pub fn dial_address(&self) -> String {
        match self {
            Endpoint::Tcp { host_port } => host_port.clone(),
            Endpoint::WebSocket { host, port, .. } => join_host_port(host, *port),
        }
    }

    pub fn is_websocket(&self) -> bool {
        matches!(self, Endpoint::WebSocket { .. })
    }
}

pub fn check_public_address(given: &str) -> Result<(), String> {
    let host = match Endpoint::parse(given)? {
        Endpoint::Tcp { host_port } => parse_host_port(&host_port)?.0,
        Endpoint::WebSocket { host, .. } => host,
    };
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) if ip.is_unspecified() => Err(format!(
            "{host} means every interface, not an address clients can connect to"
        )),
        _ => Ok(()),
    }
}

pub fn with_default_port(s: &str, port: u16) -> String {
    let bracketed = s.starts_with('[') && s.ends_with(']');
    if !s.is_empty() && (bracketed || !s.contains(':')) {
        format!("{s}:{port}")
    } else {
        s.to_string()
    }
}

fn has_port(authority: &str) -> bool {
    match authority.rsplit_once(':') {
        Some((h, p)) => {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (!h.contains(':') || h.ends_with(']'))
        }
        None => false,
    }
}

pub fn join_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::Tcp { host_port } => f.write_str(host_port),
            Endpoint::WebSocket {
                tls,
                host,
                port,
                path,
            } => {
                let scheme = if *tls { "wss" } else { "ws" };
                let default = if *tls { 443 } else { 80 };
                let host = if host.contains(':') {
                    format!("[{host}]")
                } else {
                    host.clone()
                };
                if *port == default {
                    write!(f, "{scheme}://{host}{path}")
                } else {
                    write!(f, "{scheme}://{host}:{port}{path}")
                }
            }
        }
    }
}

pub fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 1024
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~/!$&'()*+,;=:@%".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_in_a_tcp_address_never_appears_in_the_error() {
        let e = Endpoint::parse("admin:hunter2PASSW@h.example:7835").unwrap_err();
        assert!(!e.contains("hunter2"), "{e}");
    }

    #[test]
    fn only_paths_an_http_request_can_carry_are_valid() {
        assert!(valid_path("/burrow"));
        assert!(valid_path("/a/b-c_d.e~f"));
        for bad in [
            "burrow", "/a b", "/a?b", "/a#b", "/a`b", "/a<b>", "/a\tb", "/a\"b",
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
        assert!(!valid_path(&format!("/{}", "a".repeat(1024))));
    }

    fn ws(tls: bool, host: &str, port: u16, path: &str) -> Endpoint {
        Endpoint::WebSocket {
            tls,
            host: host.into(),
            port,
            path: path.into(),
        }
    }

    #[test]
    fn parses_forms() {
        assert_eq!(
            Endpoint::parse("myserver.com:2333").unwrap(),
            Endpoint::Tcp {
                host_port: "myserver.com:2333".into()
            }
        );
        assert_eq!(
            Endpoint::parse("wss://t.example.com/burrow").unwrap(),
            ws(true, "t.example.com", 443, "/burrow")
        );
        assert_eq!(
            Endpoint::parse("ws://127.0.0.1:8080/x/y").unwrap(),
            ws(false, "127.0.0.1", 8080, "/x/y")
        );
        assert_eq!(Endpoint::parse("WSS://h").unwrap(), ws(true, "h", 443, "/"));
        assert_eq!(
            Endpoint::parse("wss://[::1]:9443/b").unwrap(),
            ws(true, "::1", 9443, "/b")
        );
        assert_eq!(
            Endpoint::parse("wss://[::1]/b").unwrap(),
            ws(true, "::1", 443, "/b")
        );
    }

    #[test]
    fn rejects_bad() {
        assert!(Endpoint::parse("https://h/x").is_err());
        assert!(Endpoint::parse("wss://user:pw@h/x").is_err());
        assert!(Endpoint::parse("wss://h/x?y=1").is_err());
        assert!(Endpoint::parse("wss:///x").is_err());
        assert!(Endpoint::parse("wss://::1/x").is_err());
        assert!(Endpoint::parse("myserver.com:").is_err());
        assert!(
            Endpoint::parse("wss://h:/x")
                .unwrap_err()
                .contains("missing the port")
        );
        assert!(Endpoint::parse("::1").is_err());
        assert_eq!(Endpoint::parse(" ").unwrap_err(), "the address is empty");
    }

    #[test]
    fn host_alone_uses_the_default_port() {
        for (given, want) in [
            ("myserver.com", "myserver.com:7835"),
            ("203.0.113.5", "203.0.113.5:7835"),
            ("[2001:db8::1]", "[2001:db8::1]:7835"),
            (" myserver.com ", "myserver.com:7835"),
            ("myserver.com:2333", "myserver.com:2333"),
        ] {
            assert_eq!(
                Endpoint::parse(given).unwrap(),
                Endpoint::Tcp {
                    host_port: want.into()
                }
            );
        }
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "wss://t.example.com/burrow",
            "ws://127.0.0.1:8080/x",
            "wss://[::1]:9443/b",
            "h:1",
        ] {
            assert_eq!(Endpoint::parse(s).unwrap().to_string(), s);
        }
        assert_eq!(
            Endpoint::parse("wss://h:443/p").unwrap().to_string(),
            "wss://h/p"
        );
        assert_eq!(
            Endpoint::parse("wss://[::1]/b").unwrap().dial_address(),
            "[::1]:443"
        );
    }
}
