use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use super::{Status, human};
use crate::config::env::EnvSource;
use crate::config::{Loaded, Role, load_file};
use crate::failure::Failure;

const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY: u64 = 16 * 1024 * 1024;

pub fn reachable(listen: SocketAddr) -> SocketAddr {
    match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::from((Ipv4Addr::LOCALHOST, listen.port()))
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::from((Ipv6Addr::LOCALHOST, listen.port()))
        }
        _ => listen,
    }
}

pub fn fetch(addr: SocketAddr) -> Result<Status, String> {
    let mut s = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(TIMEOUT))
        .map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(TIMEOUT))
        .map_err(|e| e.to_string())?;
    s.write_all(
        format!("GET /status HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.take(MAX_BODY)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("the answer is not HTTP")?;
    let status_line = String::from_utf8_lossy(&raw[..split]);
    if !status_line.starts_with("HTTP/1.1 200") {
        return Err(format!(
            "it answered {}",
            status_line.lines().next().unwrap_or("nothing")
        ));
    }
    serde_json::from_slice(&raw[split + 4..])
        .map_err(|e| format!("the answer is not burrow status: {e}"))
}

pub fn run(
    config: &Path,
    json: bool,
    env: &dyn EnvSource,
    out: &mut dyn Write,
) -> Result<u8, Failure> {
    let outcome = load_file(config, Some(Role::Server), env).map_err(|e| {
        let (m, h) = e.describe(config);
        Failure::new(m).hint(h)
    })?;
    let Some(Loaded::Server(settings, _)) = outcome.loaded else {
        return Err(
            Failure::new(format!("{} is not a valid server config", config.display())).hint(
                format!(
                    "check it with: {}",
                    crate::failure::cmd(&format!("validate -c {}", config.display()))
                ),
            ),
        );
    };
    let Some(listen) = settings.metrics else {
        return Err(Failure::new(format!(
            "burrow status reads the server's metrics endpoint, which {} does not turn on",
            config.display()
        ))
        .hint("add to the server config, then reload or restart the server:\n         metrics:\n           listen: 127.0.0.1:9100"));
    };
    let addr = reachable(listen);
    let status = fetch(addr).map_err(|e| {
        Failure::new(format!("cannot read the server status from {addr}: {e}")).hint(format!(
            "is the server running with this config? start it with: {}",
            crate::failure::cmd(&format!("server -c {}", config.display()))
        ))
    })?;
    let text = if json {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&status).expect("status serializes")
        )
    } else {
        human(&status)
    };
    out.write_all(text.as_bytes())
        .map_err(|e| Failure::new(format!("cannot write output: {e}")))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_listeners_are_asked_on_loopback() {
        let sa = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert_eq!(reachable(sa("0.0.0.0:9100")), sa("127.0.0.1:9100"));
        assert_eq!(reachable(sa("[::]:9100")), sa("[::1]:9100"));
        assert_eq!(reachable(sa("10.0.0.5:9100")), sa("10.0.0.5:9100"));
    }
}
