use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use super::core::ports::{Live, LiveSession};
use super::own_update::OwnUpdate;
use crate::config::settings::ServerSettings;
use crate::metrics::Status;
use crate::server::Shared;

pub struct ServerLive {
    shared: Arc<Shared>,
    own: Option<Arc<dyn OwnUpdate>>,
}

impl ServerLive {
    pub fn new(shared: Arc<Shared>, own: Option<Arc<dyn OwnUpdate>>) -> ServerLive {
        ServerLive { shared, own }
    }
}

impl Live for ServerLive {
    fn status(&self) -> Status {
        self.shared.status()
    }

    fn settings(&self) -> Arc<ServerSettings> {
        self.shared.current_settings()
    }

    fn sessions(&self) -> Vec<LiveSession> {
        self.shared
            .client_sessions()
            .into_iter()
            .map(|s| LiveSession {
                client: s.name,
                secret_fp: s.secret_fp,
                from: s.from,
                over_websocket: s.over_websocket,
            })
            .collect()
    }

    fn allows(&self) -> BTreeMap<String, Vec<String>> {
        self.shared
            .client_sessions()
            .into_iter()
            .filter_map(|s| s.allow.map(|a| (s.name, a)))
            .collect()
    }

    fn bind_probe(&self, proto: burrow_config::Protocol, addr: SocketAddr) -> std::io::Result<()> {
        super::adapters::probe_bind(proto, addr)
    }

    fn latest(&self) -> burrow_admin_proto::updates::LatestRelease {
        super::latest::cached()
    }

    fn latest_now(&self) -> burrow_admin_proto::updates::LatestRelease {
        super::latest::fresh()
    }

    fn ask_update(&self, client: &str, to: &str, by: &str) -> Result<(), String> {
        self.shared.ask_update(client, to, by)
    }

    fn update_all(&self, clients: Vec<String>, to: &str, by: &str) -> Result<(), String> {
        self.shared.update_all(clients, to, by)
    }

    fn own_update(&self) -> Option<burrow_admin_proto::updates::ServerUpdate> {
        self.own
            .as_ref()
            .map(|o| o.state(&self.shared.current_settings()))
    }

    fn update_self(&self, by: &str) -> Result<(), String> {
        match &self.own {
            Some(o) => o.ask(&self.shared.current_settings(), by),
            None => Err("this server cannot update itself from burrow admin".into()),
        }
    }

    fn free_port(&self, near: u16, count: u16, udp: bool) -> Option<u16> {
        let mut p = near.max(1024);
        while p.checked_add(count)?.checked_sub(1).is_some() {
            let all = (0..count).all(|i| {
                let addr = SocketAddr::from(([0, 0, 0, 0], p + i));
                if udp {
                    std::net::UdpSocket::bind(addr).is_ok()
                } else {
                    std::net::TcpListener::bind(addr).is_ok()
                }
            });
            if all {
                return Some(p);
            }
            p = p.checked_add(count)?;
        }
        None
    }
}
