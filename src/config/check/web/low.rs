use std::net::SocketAddr;

use super::Spans;
use crate::config::diagnostics::Diagnostic;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Os {
    Linux,
    Mac,
    OtherUnix,
    Windows,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Rights {
    pub(super) os: Os,
    pub(super) root: bool,
    pub(super) bind_cap: bool,
    pub(super) unprivileged_from: u16,
}

pub(super) fn needs_rights(addr: SocketAddr, r: Rights) -> bool {
    let low = addr.port() != 0 && addr.port() < r.unprivileged_from;
    match r.os {
        Os::Windows => false,
        _ if r.root || !low => false,
        Os::Linux => !r.bind_cap,
        Os::Mac => !addr.ip().is_unspecified(),
        Os::OtherUnix => true,
    }
}

fn os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else if cfg!(target_os = "macos") {
        Os::Mac
    } else {
        Os::OtherUnix
    }
}

#[cfg(unix)]
fn root() -> bool {
    rustix::process::geteuid().is_root()
}

#[cfg(not(unix))]
fn root() -> bool {
    false
}

fn rights() -> Rights {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    let linux = os() == Os::Linux;
    let bind_cap = linux
        && read("/proc/self/status")
            .lines()
            .find_map(|l| l.strip_prefix("CapEff:"))
            .and_then(|h| u64::from_str_radix(h.trim(), 16).ok())
            .is_some_and(|bits| bits & (1 << 10) != 0);
    let unprivileged_from = if linux {
        read("/proc/sys/net/ipv4/ip_unprivileged_port_start")
            .trim()
            .parse()
            .unwrap_or(1024)
    } else {
        1024
    };
    Rights {
        os: os(),
        root: root(),
        bind_cap,
        unprivileged_from,
    }
}

pub(super) fn message(key: &str, addr: SocketAddr, os: Os) -> String {
    let who = match os {
        Os::Linux => "root or CAP_NET_BIND_SERVICE",
        _ => "root",
    };
    format!(
        "web.{key} {addr}: port {} needs {who}; burrow service install grants it, or run burrow as root",
        addr.port()
    )
}

pub(super) fn help(port: u16) -> String {
    format!(
        "Let's Encrypt checks only ports 443 and 80 from the internet, so keep port {port} for them; to run burrow on a higher port behind a router, see \"Port forwarding at home\" in docs/web.md"
    )
}

pub(in crate::config::check) fn check_low_ports(
    (https, http): (Option<SocketAddr>, Option<SocketAddr>),
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let r = rights();
    for (key, addr) in [("https", https), ("http", http)] {
        let Some(addr) = addr.filter(|a| needs_rights(*a, r)) else {
            continue;
        };
        out.push(
            Diagnostic::warning("low-port", message(key, addr, r.os))
                .at(spans.value(&["web", key]))
                .help(help(addr.port())),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(os: Os, root: bool, cap: bool) -> Rights {
        Rights {
            os,
            root,
            bind_cap: cap,
            unprivileged_from: 1024,
        }
    }

    #[test]
    fn a_low_web_port_needs_rights_where_the_system_asks_for_them() {
        let any: SocketAddr = "[::]:443".parse().unwrap();
        let one: SocketAddr = "203.0.113.10:443".parse().unwrap();
        let high: SocketAddr = "[::]:8443".parse().unwrap();
        assert!(needs_rights(any, r(Os::Linux, false, false)));
        assert!(!needs_rights(any, r(Os::Linux, false, true)));
        assert!(!needs_rights(any, r(Os::Linux, true, false)));
        assert!(!needs_rights(high, r(Os::Linux, false, false)));
        let lowered = Rights {
            unprivileged_from: 80,
            ..r(Os::Linux, false, false)
        };
        assert!(!needs_rights(any, lowered));
        assert!(!needs_rights(any, r(Os::Mac, false, false)));
        assert!(needs_rights(one, r(Os::Mac, false, false)));
        assert!(needs_rights(any, r(Os::OtherUnix, false, false)));
        assert!(!needs_rights(any, r(Os::Windows, false, false)));
        assert_eq!(
            message("https", any, Os::Linux),
            "web.https [::]:443: port 443 needs root or CAP_NET_BIND_SERVICE; burrow service install grants it, or run burrow as root"
        );
        assert_eq!(
            help(80),
            "Let's Encrypt checks only ports 443 and 80 from the internet, so keep port 80 for them; to run burrow on a higher port behind a router, see \"Port forwarding at home\" in docs/web.md"
        );
    }
}
