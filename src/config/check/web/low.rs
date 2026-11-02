use std::net::SocketAddr;

use super::Spans;
use crate::base::ports::{Os, low_port_who, os};
use crate::config::diagnostics::Diagnostic;

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

pub(super) fn message(key: &str, port: u16, os: Os) -> String {
    format!("{key}: port {port} needs {}", low_port_who(os))
}

pub(super) fn help(key: &[String], port: u16, fix: &str) -> String {
    if key.first().is_some_and(|k| k == "web") {
        format!(
            "{fix}; Let's Encrypt checks only ports 443 and 80, so a higher port needs the router to forward {port} to it (docs/web/proxies.md)"
        )
    } else {
        fix.to_string()
    }
}

pub(crate) fn refused_here(addr: SocketAddr) -> bool {
    needs_rights(addr, rights())
}

pub(in crate::config::check) fn check_low_ports(
    binds: &[(Vec<String>, SocketAddr)],
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let r = rights();
    for (path, addr) in binds.iter().filter(|(_, a)| needs_rights(*a, r)) {
        let at: Vec<&str> = path.iter().map(String::as_str).collect();
        out.push(
            Diagnostic::warning("low-port", message(&path.join("."), addr.port(), r.os))
                .at(spans.value(&at))
                .help(help(
                    path,
                    addr.port(),
                    &crate::base::ports::low_port_hint(),
                )),
        );
    }
}

pub(super) fn swapped(key: &str, port: u16) -> Option<(String, &'static str)> {
    let (other, scheme) = match (key, port) {
        ("https", 80) => ("80", "http://"),
        ("http", 443) => ("443", "https://"),
        _ => return None,
    };
    Some((
        format!(
            "web.{key}: port {other} is where browsers send {scheme} addresses, so visitors who type a name do not reach these websites"
        ),
        "use 443 for HTTPS and 80 for HTTP; keep this only when a router or proxy in front sends the usual port here",
    ))
}

pub(in crate::config::check) fn check_swapped(
    (https, http): (Option<SocketAddr>, Option<SocketAddr>),
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    for (key, addr) in [("https", https), ("http", http)] {
        if let Some((message, help)) = addr.and_then(|a| swapped(key, a.port())) {
            out.push(
                Diagnostic::warning("swapped-port", message)
                    .at(spans.value(&["web", key]))
                    .help(help),
            );
        }
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
    fn only_the_two_well_known_ports_count_as_swapped() {
        assert!(swapped("https", 80).is_some());
        assert!(swapped("http", 443).is_some());
        for (key, port) in [
            ("https", 443),
            ("http", 80),
            ("https", 8080),
            ("http", 8443),
            ("http", 4432),
            ("https", 8000),
        ] {
            assert!(swapped(key, port).is_none(), "{key} {port}");
        }
        assert_eq!(
            swapped("https", 80).unwrap().0,
            "web.https: port 80 is where browsers send http:// addresses, so visitors who type a name do not reach these websites"
        );
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
            message("web.https", 443, Os::Linux),
            "web.https: port 443 needs root or CAP_NET_BIND_SERVICE"
        );
        assert_eq!(
            message("services.ssh.bind", 22, Os::OtherUnix),
            "services.ssh.bind: port 22 needs root"
        );
        assert_eq!(help(&["listen".into()], 22, "fix"), "fix");
        assert_eq!(
            help(
                &["web".into(), "http".into()],
                80,
                "run burrow as root, or use a port above 1023"
            ),
            "run burrow as root, or use a port above 1023; Let's Encrypt checks only ports 443 and 80, so a higher port needs the router to forward 80 to it (docs/web/proxies.md)"
        );
    }
}
