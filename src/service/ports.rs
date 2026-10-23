use std::net::SocketAddr;

use crate::failure::Failure;
use burrow_config::Protocol;

pub struct PortUser {
    pub pid: u32,
    pub name: String,
    pub managed: bool,
}

pub fn check_free(
    proto: Protocol,
    addr: SocketAddr,
    label: &str,
    users: &dyn Fn() -> Vec<PortUser>,
) -> Result<(), Failure> {
    let kind = match proto {
        Protocol::Tcp => socket2::Type::STREAM,
        Protocol::Udp => socket2::Type::DGRAM,
    };
    let result = crate::transport::udp::std_socket(addr, kind).map(drop);
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            let owner = owner(proto, addr.port());
            let user = owner
                .as_ref()
                .and_then(|(_, pid)| users().into_iter().find(|u| u.pid == *pid));
            let busy = format!(
                "{} port {} ({label}) is already in use",
                proto.wire(),
                addr.port()
            );
            let see = inspect_hint(proto, addr.port());
            Err(match (owner, user) {
                (Some((_, pid)), Some(u)) if u.managed => Failure::new(format!(
                    "{busy} by service {} (pid {pid})",
                    u.name
                ))
                .hint(format!(
                    "two services cannot use one port; pick another port in the config, or stop {} first: {}",
                    u.name,
                    crate::failure::admin(&format!("service stop {}", u.name))
                )),
                (Some((program, pid)), Some(u)) => Failure::new(format!(
                    "{busy} by {program} (pid {pid}), from {} (not a service burrow manages)",
                    u.name
                ))
                .hint(format!(
                    "pick another port in the config, or stop that program; see: {see}"
                )),
                (owner, _) => Failure::new(format!(
                    "{busy}{}",
                    owner
                        .map(|(program, pid)| format!(" by {program} (pid {pid})"))
                        .unwrap_or_default()
                ))
                .hint(format!(
                    "pick another port in the config, or stop that program; see: {see}"
                )),
            })
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
    } else if cfg!(target_os = "linux") {
        let flag = if proto == Protocol::Udp {
            "-lunp"
        } else {
            "-ltnp"
        };
        format!("sudo ss {flag} 'sport = :{port}'")
    } else if cfg!(target_os = "freebsd") {
        let flag = proto.wire();
        format!("sockstat -46 -l -P {flag} -p {port}")
    } else {
        let flag = if proto == Protocol::Udp { "UDP" } else { "TCP" };
        format!("sudo lsof -nP -i{flag}:{port}")
    }
}

#[cfg(target_os = "linux")]
fn owner(proto: Protocol, port: u16) -> Option<(String, u32)> {
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
                return Some((comm.trim().to_string(), pid.parse().ok()?));
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn owner(proto: Protocol, port: u16) -> Option<(String, u32)> {
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
    Some((cmd.to_string(), pid.parse().ok()?))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn owner(_: Protocol, _: u16) -> Option<(String, u32)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_port_is_reported_with_its_owner() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let err = check_free(Protocol::Tcp, addr, "service \"web\"", &Vec::new).unwrap_err();
        assert!(
            err.message.contains(&format!(
                "tcp port {} (service \"web\") is already in use",
                addr.port()
            )),
            "{}",
            err.message
        );
        #[cfg(target_os = "linux")]
        assert!(err.message.contains("pid "), "{}", err.message);
        drop(l);
        let started = std::time::Instant::now();
        while check_free(Protocol::Tcp, addr, "x", &Vec::new).is_err() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "port {} stays busy after its listener closed",
                addr.port()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_port_held_by_an_installed_service_names_it() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let me = std::process::id();
        let user = |managed: bool| {
            move || {
                vec![PortUser {
                    pid: me,
                    name: if managed {
                        "burrow-server".into()
                    } else {
                        "burrow-server.service".into()
                    },
                    managed,
                }]
            }
        };
        let err = check_free(Protocol::Tcp, addr, "listen", &user(true)).unwrap_err();
        assert_eq!(
            err.message,
            format!(
                "tcp port {} (listen) is already in use by service burrow-server (pid {me})",
                addr.port()
            )
        );
        let hint = err.hint.unwrap();
        assert!(hint.contains("service stop burrow-server"), "{hint}");
        let err = check_free(Protocol::Tcp, addr, "listen", &user(false)).unwrap_err();
        assert!(
            err.message.ends_with(&format!(
                "(pid {me}), from burrow-server.service (not a service burrow manages)"
            )),
            "{}",
            err.message
        );
        let err = check_free(Protocol::Tcp, addr, "listen", &Vec::new).unwrap_err();
        assert!(
            err.message.ends_with(&format!("(pid {me})")),
            "{}",
            err.message
        );
    }

    #[test]
    fn foreign_address_is_explained() {
        let err = check_free(
            Protocol::Tcp,
            "203.0.113.7:7840".parse().unwrap(),
            "service \"web\"",
            &Vec::new,
        )
        .unwrap_err();
        assert!(err.message.contains("is not an address of this machine"));
    }
}
