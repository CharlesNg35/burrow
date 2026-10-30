use std::net::{IpAddr, SocketAddr};

use crate::endpoint::{Endpoint, with_default_port};
use crate::hostport::parse_host_port;

#[derive(Debug, Clone, Copy)]
pub struct WsShape<'a> {
    pub listen: SocketAddr,
    pub path: &'a str,
    pub tls: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    Nowhere,
    ThisMachine,
    LocalNetwork,
}

fn reach_of_ip(ip: IpAddr) -> Option<Reach> {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            if v4.is_unspecified() {
                Some(Reach::Nowhere)
            } else if v4.is_loopback() {
                Some(Reach::ThisMachine)
            } else if v4.is_private() || v4.is_link_local() || (a == 100 && (64..128).contains(&b))
            {
                Some(Reach::LocalNetwork)
            } else {
                None
            }
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return reach_of_ip(IpAddr::V4(v4));
            }
            if v6.is_unspecified() {
                Some(Reach::Nowhere)
            } else if v6.is_loopback() {
                Some(Reach::ThisMachine)
            } else if v6.is_unicast_link_local() || (v6.segments()[0] & 0xfe00) == 0xfc00 {
                Some(Reach::LocalNetwork)
            } else {
                None
            }
        }
    }
}

pub fn is_localhost(host: &str) -> bool {
    host.trim().eq_ignore_ascii_case("localhost")
}

fn reach_of_host(host: &str) -> Option<Reach> {
    if is_localhost(host) {
        return Some(Reach::ThisMachine);
    }
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .ok()
        .and_then(reach_of_ip)
}

#[derive(Debug)]
pub struct AddressError {
    pub message: String,
    pub hint: String,
}

fn error(message: impl Into<String>, hint: impl Into<String>) -> AddressError {
    AddressError {
        message: message.into(),
        hint: hint.into(),
    }
}

pub const FORMS: &str = "use a host name or IP, optionally with :PORT, or the WebSocket URL (wss://tunnel.example.com/burrow)";

pub fn client_address(
    given: &str,
    listen: Option<SocketAddr>,
    websocket: Option<WsShape<'_>>,
    who: &str,
    notes: &mut Vec<String>,
) -> Result<Endpoint, AddressError> {
    let given = given.trim();
    let parse = |s: &str| Endpoint::parse(s).map_err(|e| error(e, FORMS));
    let endpoint = if given.contains("://") {
        parse(given)?
    } else {
        match (listen, websocket) {
            (Some(listen), _) => {
                let with_port = with_default_port(given, listen.port());
                let e = parse(&with_port)?;
                if let Ok((_, port)) = parse_host_port(&with_port)
                    && port != listen.port()
                {
                    notes.push(format!(
                        "port {port} differs from the listen port {}; this only works if a router or firewall forwards {port} to it",
                        listen.port()
                    ));
                }
                e
            }
            (None, Some(ws)) => {
                if parse_host_port(given).is_ok() {
                    return Err(error(
                        "the server only accepts WebSocket clients",
                        format!(
                            "use the host name alone, or the WebSocket URL, for example wss://tunnel.example.com{}",
                            ws.path
                        ),
                    ));
                }
                if given.contains(':') && !given.starts_with('[') {
                    return Err(error(
                        format!(
                            "\"{given}\" looks like an IPv6 address; write it in brackets, like [2001:db8::1]"
                        ),
                        FORMS,
                    ));
                }
                let port = if ws.tls { ws.listen.port() } else { 443 };
                let url = Endpoint::WebSocket {
                    tls: true,
                    host: given
                        .trim_start_matches('[')
                        .trim_end_matches(']')
                        .to_string(),
                    port,
                    path: ws.path.to_string(),
                };
                parse(&url.to_string())?
            }
            (None, None) => parse(given)?,
        }
    };
    match (&endpoint, websocket) {
        (Endpoint::WebSocket { .. }, None) => {
            return Err(error(
                "the server has no websocket section",
                "use the host name, or add a websocket section to the server config",
            ));
        }
        (Endpoint::Tcp { .. }, Some(ws)) if listen.is_none() => {
            return Err(error(
                "the server only accepts WebSocket clients",
                format!("use the WebSocket URL, for example wss://tunnel.example.com{}", ws.path),
            ));
        }
        (Endpoint::WebSocket { path, .. }, Some(ws)) if *path != ws.path => notes.push(format!(
            "the URL path {path} differs from websocket.path {}; this only works if your proxy rewrites the path",
            ws.path
        )),
        _ => {}
    }
    let host = match &endpoint {
        Endpoint::Tcp { host_port } => parse_host_port(host_port)
            .map(|(h, _)| h)
            .unwrap_or_default(),
        Endpoint::WebSocket { host, .. } => host.clone(),
    };
    match reach_of_host(&host) {
        Some(Reach::Nowhere) => {
            return Err(error(
                format!("{host} means every interface, not an address clients can connect to"),
                "use the host name or IP clients connect to, for example myserver.example.com",
            ));
        }
        Some(Reach::ThisMachine) => notes.push(format!(
            "{host} is this machine: only {who} running on it can connect"
        )),
        Some(Reach::LocalNetwork) => notes.push(format!(
            "{host} is a private address: only {who} on the same network or VPN can connect"
        )),
        None => {}
    }
    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tcp(given: &str, listen: &str) -> Result<(String, Vec<String>), AddressError> {
        let mut notes = Vec::new();
        client_address(
            given,
            Some(listen.parse().unwrap()),
            None,
            "clients",
            &mut notes,
        )
        .map(|e| (e.to_string(), notes))
    }

    #[test]
    fn host_takes_the_listen_port() {
        assert_eq!(
            tcp("myserver.example.com", "0.0.0.0:2333").unwrap().0,
            "myserver.example.com:2333"
        );
        assert_eq!(
            tcp("[2001:db8::1]", "[::]:7835").unwrap().0,
            "[2001:db8::1]:7835"
        );
        let (e, notes) = tcp("myserver.example.com:9000", "0.0.0.0:7835").unwrap();
        assert_eq!(e, "myserver.example.com:9000");
        assert!(notes[0].contains("port 9000 differs"), "{notes:?}");
    }

    #[test]
    fn public_hosts_have_no_notes() {
        for host in ["myserver.example.com", "203.0.113.5", "[2001:db8::1]"] {
            assert!(tcp(host, "0.0.0.0:7835").unwrap().1.is_empty(), "{host}");
        }
    }

    #[test]
    fn local_hosts_get_a_note() {
        for (host, want) in [
            ("localhost", "only clients running on it"),
            ("127.0.0.1", "only clients running on it"),
            ("[::1]", "only clients running on it"),
            ("192.168.1.10", "same network or VPN"),
            ("10.0.0.5", "same network or VPN"),
            ("172.16.0.1", "same network or VPN"),
            ("100.64.1.2", "same network or VPN"),
            ("169.254.1.1", "same network or VPN"),
            ("[fd00::1]", "same network or VPN"),
            ("[fe80::1]", "same network or VPN"),
        ] {
            let (_, notes) = tcp(host, "0.0.0.0:7835").unwrap();
            assert!(
                notes.len() == 1 && notes[0].contains(want),
                "{host}: {notes:?}"
            );
        }
        assert!(tcp("100.128.0.1", "0.0.0.0:7835").unwrap().1.is_empty());
    }

    #[test]
    fn unusable_addresses_are_refused() {
        for bad in ["0.0.0.0", "[::]", "myserver.com:", "a b", "", "::1"] {
            assert!(tcp(bad, "0.0.0.0:7835").is_err(), "{bad}");
        }
    }
}
