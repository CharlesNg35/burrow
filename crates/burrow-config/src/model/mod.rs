mod access_keys;
mod certs;
mod client;
mod client_ref;
mod dns;
mod entries;
mod options;
mod server;
mod site;
mod site_keys;
mod visitor;
mod web;

pub use access_keys::{
    ClientAuthFiles, ForwardAuthConfig, ForwardAuthSetting, LoginPick, RESERVED_LOGIN_NAMES,
};
pub use certs::{CertFile, Directory, DirectoryName, LetsEncrypt};
pub use client::{
    CLIENT_MAX_CONNECTIONS, CLIENT_MAX_UDP_PEERS, Cipher, ClientConfig, OnRejected, Reconnect,
};
pub use client_ref::{ClientRef, WEIGHT_MAX, WeightedClient, ref_name};
pub use dns::{AcmeDnsDns, CloudflareDns, DnsEntry, Rfc2136Dns};
pub use entries::{
    Bind, ClientEntry, HealthCheck, HealthCheckSetting, Mode, Protocol, ProxyVersion, Reach,
    Secrets, ServiceEntry, Target, VisitorEntry,
};
pub use options::{ClientUdpOptions, Heartbeat, Limits, Pool, TcpOptions, UdpOptions};
pub use server::{
    MetricsConfig, SelfEntry, ServerConfig, TlsFiles, WebSocketConfig, default_ws_path,
};
pub use site::{ChallengeKind, SiteFiles, SiteTls, TlsMode, WebSite};
pub use site_keys::{
    AccessLog, AccessLogFormat, AccessLogOptions, RedirectRule, RedirectTarget, TargetCa,
    TargetTls, TargetUnchecked,
};
pub use visitor::{ForwardEntry, VisitorConfig, VisitorUdpOptions};
pub use web::{ServiceWeb, WebConfig};

fn is_false(v: &bool) -> bool {
    !v
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_schema::AdminEnum;
    use burrow_admin_proto::msg::AdminLevel;
    use burrow_admin_proto::serde_json::{self, json};

    #[test]
    fn the_admin_level_options_are_its_wire_values() {
        let options = AdminLevel::options();
        assert_eq!(options.len(), 2);
        for o in options {
            let l: AdminLevel = serde_json::from_value(json!(o.value)).unwrap();
            assert_eq!(l.wire(), o.value);
            assert_eq!(AdminLevel::from_wire(&o.value), Some(l));
            assert_eq!(serde_json::to_value(l).unwrap(), json!(o.value));
        }
        assert_eq!(AdminLevel::from_wire("owner"), None);
    }

    #[test]
    fn a_client_keeps_16384_connections_unless_told_otherwise() {
        let base = json!({"server": "tunnel.example.com", "name": "home", "secret": "x"});
        let plain: ClientConfig = serde_json::from_value(base.clone()).unwrap();
        assert_eq!(plain.max_connections, 16384);
        assert!(
            serde_json::to_value(&plain)
                .unwrap()
                .get("max_connections")
                .is_none()
        );
        let mut set = base;
        set["max_connections"] = json!(2);
        let two: ClientConfig = serde_json::from_value(set).unwrap();
        assert_eq!(two.max_connections, 2);
        assert_eq!(serde_json::to_value(&two).unwrap()["max_connections"], 2);
    }

    #[test]
    fn a_client_keeps_8192_udp_peers_unless_told_otherwise() {
        let base = json!({"server": "tunnel.example.com", "name": "home", "secret": "x"});
        let plain: ClientConfig = serde_json::from_value(base.clone()).unwrap();
        assert_eq!(plain.max_udp_peers, 8192);
        assert!(
            serde_json::to_value(&plain)
                .unwrap()
                .get("max_udp_peers")
                .is_none()
        );
        let mut set = base;
        set["max_udp_peers"] = json!(3);
        let three: ClientConfig = serde_json::from_value(set).unwrap();
        assert_eq!(three.max_udp_peers, 3);
        assert_eq!(serde_json::to_value(&three).unwrap()["max_udp_peers"], 3);
        let udp: Result<ClientConfig, _> = serde_json::from_value(
            json!({"server": "a", "name": "b", "secret": "c", "udp": {"max_peers": 5}}),
        );
        assert!(udp.is_err());
    }

    #[test]
    fn a_server_keeps_4096_udp_peers_per_port_unless_told_otherwise() {
        let plain = UdpOptions::default();
        assert_eq!(plain.max_peers, 4096);
        let set: UdpOptions = serde_json::from_value(json!({"max_peers": 7})).unwrap();
        assert_eq!(set.max_peers, 7);
        assert_eq!(serde_json::to_value(&set).unwrap()["max_peers"], 7);
    }

    #[test]
    fn udp_peers_are_kept_two_minutes_by_default_as_rfc_4787_req_5_asks() {
        assert_eq!(UdpOptions::default().session_timeout_secs, 120);
        assert_eq!(ClientUdpOptions::default().session_timeout_secs, 120);
        assert_eq!(VisitorUdpOptions::default().session_timeout_secs, 120);
        assert_eq!(VisitorUdpOptions::default().max_peers, 4096);
        let set: UdpOptions = serde_json::from_value(json!({"max_peers": 7})).unwrap();
        assert_eq!(set.session_timeout_secs, 120);
    }
}
