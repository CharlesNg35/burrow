use std::future::Future;
use std::time::Duration;

use burrow_config::Protocol;

pub(crate) const RETRY_GAP: Duration = Duration::from_millis(100);

pub(crate) async fn retry<T, E, F, Fut>(tries: usize, mut attempt: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut tries = tries.max(1);
    loop {
        match attempt().await {
            Ok(v) => return Ok(v),
            Err(e) if tries <= 1 => return Err(e),
            Err(_) => tries -= 1,
        }
        tokio::time::sleep(RETRY_GAP).await;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Os {
    Linux,
    Mac,
    OtherUnix,
    Windows,
}

pub(crate) fn os() -> Os {
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

pub(crate) fn low_port_who(os: Os) -> &'static str {
    match os {
        Os::Linux => "root or CAP_NET_BIND_SERVICE",
        Os::Mac => "root, except on all addresses (0.0.0.0 or ::)",
        _ => "root",
    }
}

pub(crate) fn low_port_fix(os: Os, burrow: &str, exe: &str) -> String {
    match os {
        Os::Linux => format!(
            "by hand: sudo setcap cap_net_bind_service=+ep {exe}, again after each update; as a service, {burrow} service install grants it; or use a port above 1023"
        ),
        Os::Mac => "listen on all addresses, use a port above 1023, or run burrow as root".into(),
        _ => "run burrow as root, or use a port above 1023".into(),
    }
}

pub(crate) fn not_here_hint() -> String {
    format!(
        "use 0.0.0.0, ::, or an IP address assigned to this machine (see: {})",
        crate::base::failure::addresses_cmd()
    )
}

pub(crate) fn low_port_service_fix(os: Os, reinstall: &str) -> String {
    match os {
        Os::Linux => format!(
            "this service was installed without it; install it again to grant it, nothing moves: {reinstall}; or use a port above 1023"
        ),
        Os::Mac => format!(
            "listen on all addresses, use a port above 1023, or install it again as root: {reinstall} --run-as root"
        ),
        _ => "run burrow as root, or use a port above 1023".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_low_port_hint_names_the_service_and_setcap_on_linux() {
        assert_eq!(
            low_port_fix(Os::Linux, "burrow", "/usr/local/bin/burrow"),
            "by hand: sudo setcap cap_net_bind_service=+ep /usr/local/bin/burrow, again after each update; as a service, burrow service install grants it; or use a port above 1023"
        );
        assert_eq!(
            low_port_service_fix(
                Os::Linux,
                "sudo burrow service install -c /etc/burrow/home/server.yaml"
            ),
            "this service was installed without it; install it again to grant it, nothing moves: sudo burrow service install -c /etc/burrow/home/server.yaml; or use a port above 1023"
        );
        assert_eq!(
            low_port_fix(Os::OtherUnix, "burrow", "/x"),
            "run burrow as root, or use a port above 1023"
        );
        assert_eq!(low_port_who(Os::Linux), "root or CAP_NET_BIND_SERVICE");
    }

    #[tokio::test]
    async fn a_bind_is_tried_the_given_number_of_times_then_its_last_error_is_kept() {
        let mut calls = 0;
        let r: Result<(), usize> = retry(3, || {
            calls += 1;
            let n = calls;
            async move { Err(n) }
        })
        .await;
        assert_eq!(r, Err(3));
        let mut calls = 0;
        let r: Result<usize, usize> = retry(5, || {
            calls += 1;
            let n = calls;
            async move { if n == 2 { Ok(n) } else { Err(n) } }
        })
        .await;
        assert_eq!(r, Ok(2));
        let r: Result<(), &str> = retry(0, || async { Err("once") }).await;
        assert_eq!(r, Err("once"));
    }
}
