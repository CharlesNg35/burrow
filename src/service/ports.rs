use std::net::SocketAddr;

use crate::config::model::Protocol;
use crate::failure::Failure;

pub fn check_free(proto: Protocol, addr: SocketAddr, label: &str) -> Result<(), Failure> {
    let kind = match proto {
        Protocol::Tcp => socket2::Type::STREAM,
        Protocol::Udp => socket2::Type::DGRAM,
    };
    let result = crate::transport::udp::std_socket(addr, kind).map(drop);
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            let owner = owner(proto, addr.port())
                .map(|o| format!(" by {o}"))
                .unwrap_or_default();
            Err(Failure::new(format!(
                "{} port {} ({label}) is already in use{owner}",
                proto.as_str(),
                addr.port()
            ))
            .hint(format!(
                "pick another port in the config, or stop that program; see: {}",
                inspect_hint(proto, addr.port())
            )))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrNotAvailable => Err(Failure::new(format!(
            "{addr} ({label}) is not an address of this machine"
        ))
        .hint(format!(
            "use 0.0.0.0, 127.0.0.1 or an IP shown by: {}",
            crate::failure::addresses_cmd()
        ))),
        Err(_) => Ok(()),
    }
}

pub(crate) fn inspect_hint(proto: Protocol, port: u16) -> String {
    if cfg!(windows) {
        format!("netstat -ano | findstr :{port}")
    } else if cfg!(target_os = "macos") {
        let flag = if proto == Protocol::Udp { "UDP" } else { "TCP" };
        format!("sudo lsof -nP -i{flag}:{port}")
    } else {
        let flag = if proto == Protocol::Udp {
            "-lunp"
        } else {
            "-ltnp"
        };
        format!("sudo ss {flag} 'sport = :{port}'")
    }
}

#[cfg(target_os = "linux")]
fn owner(proto: Protocol, port: u16) -> Option<String> {
    let tables: &[&str] = match proto {
        Protocol::Tcp => &["/proc/net/tcp", "/proc/net/tcp6"],
        Protocol::Udp => &["/proc/net/udp", "/proc/net/udp6"],
    };
    let mut inodes = Vec::new();
    for table in tables {
        let Ok(text) = std::fs::read_to_string(table) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 10 {
                continue;
            }
            let local_port = cols[1]
                .rsplit(':')
                .next()
                .and_then(|p| u16::from_str_radix(p, 16).ok());
            let listening = proto == Protocol::Udp || cols[3] == "0A";
            if local_port == Some(port) && listening {
                inodes.push(format!("socket:[{}]", cols[9]));
            }
        }
    }
    if inodes.is_empty() {
        return None;
    }
    for proc in std::fs::read_dir("/proc").ok()?.flatten() {
        let pid = proc.file_name().to_string_lossy().into_owned();
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(proc.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(target) = std::fs::read_link(fd.path())
                && inodes.iter().any(|i| target.as_os_str() == i.as_str())
            {
                let comm = std::fs::read_to_string(proc.path().join("comm")).unwrap_or_default();
                return Some(format!("{} (pid {pid})", comm.trim()));
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn owner(proto: Protocol, port: u16) -> Option<String> {
    let flag = if proto == Protocol::Udp {
        format!("-iUDP:{port}")
    } else {
        format!("-iTCP:{port}")
    };
    let out = std::process::Command::new("lsof")
        .args(["-nP", &flag, "-Fpc"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pid = text.lines().find_map(|l| l.strip_prefix('p'))?;
    let cmd = text
        .lines()
        .find_map(|l| l.strip_prefix('c'))
        .unwrap_or("a program");
    Some(format!("{cmd} (pid {pid})"))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn owner(_: Protocol, _: u16) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_port_is_reported_with_its_owner() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let err = check_free(Protocol::Tcp, addr, "service \"web\"").unwrap_err();
        assert!(
            err.message.contains(&format!(
                "tcp port {} (service \"web\") is already in use",
                addr.port()
            )),
            "{}",
            err.message
        );
        #[cfg(target_os = "linux")]
        assert!(
            err.message.contains(&format!("pid {}", std::process::id())),
            "{}",
            err.message
        );
        drop(l);
        assert!(check_free(Protocol::Tcp, addr, "x").is_ok());
    }

    #[test]
    fn foreign_address_is_explained() {
        let err = check_free(
            Protocol::Tcp,
            "203.0.113.7:7840".parse().unwrap(),
            "service \"web\"",
        )
        .unwrap_err();
        assert!(err.message.contains("is not an address of this machine"));
    }
}
