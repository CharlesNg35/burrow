use burrow_config::Protocol;

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
