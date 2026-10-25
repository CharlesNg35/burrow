use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::tcp::log_status;
use super::{Shared, TakeError};
use crate::config::settings::Service;
use crate::forward::ForwardError;
use crate::metrics::ServiceStats;
use crate::policy::ratelimit::Limit;
use crate::udp::front::{self, Ending, Front, Hooks, Interrupt, Meter};
use crate::udp::link::{Opening, open_udp};
use crate::udp::peers::{PeerCount, PeerGauge, PeerTable};
use crate::wire::protocol::{Connect, Status};
use burrow_config::Protocol;

impl PeerCount for Arc<Shared> {
    fn peers(&self) -> &std::sync::atomic::AtomicUsize {
        &self.udp_peers
    }
}

pub async fn udp_service(
    shared: Arc<Shared>,
    name: String,
    socket: UdpSocket,
    config: watch::Receiver<Service>,
    index: u16,
) {
    let socket = Arc::new(socket);
    let mut stop = shared.shutdown.clone();
    loop {
        if *stop.borrow() {
            return;
        }
        let svc = config.borrow().clone();
        let target = svc.target_at(index);
        let Some(session) = shared.pick(&name, &svc, &target) else {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => continue,
                _ = stop.changed() => return,
            }
        };
        let ch = match tokio::time::timeout(Duration::from_secs(10), session.take()).await {
            Ok(Ok(ch)) => ch,
            Ok(Err(TakeError::TooManyPending)) | Ok(Err(TakeError::Closed)) => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
            Err(_) => {
                if svc.clients.len() > 1 {
                    shared.troubled(&name, &session, &target, &crate::forward::no_reply());
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        debug!(
            "service \"{name}\": UDP relay attached to client \"{}\"",
            session.name()
        );
        let port = Port {
            name: &name,
            svc: &svc,
            target: &target,
        };
        match relay(&shared, port, &session, socket.clone(), ch, config.clone()).await {
            Ok(()) => {}
            Err(Some(status)) => {
                if status == Status::Busy {
                    shared.metrics.client_busy(session.name());
                }
                log_status(
                    &format!("service \"{name}\""),
                    session.name(),
                    &target,
                    status,
                );
                if svc.clients.len() > 1 {
                    shared.troubled(&name, &session, &target, &ForwardError::Status(status));
                }
                let pause = if svc.clients.len() > 1 && shared.has_clear(&name, &svc, &target) {
                    Duration::from_millis(200)
                } else {
                    Duration::from_secs(5)
                };
                tokio::time::sleep(pause).await;
            }
            Err(None) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}

struct Port<'a> {
    name: &'a str,
    svc: &'a Service,
    target: &'a str,
}

async fn relay(
    shared: &Arc<Shared>,
    port: Port<'_>,
    session: &super::via::Via,
    socket: Arc<UdpSocket>,
    ch: crate::wire::transport::Channel,
    config: watch::Receiver<Service>,
) -> Result<(), Option<Status>> {
    let Port { name, svc, target } = port;
    let connect = Connect {
        protocol: Protocol::Udp,
        service: name.to_string(),
        target: target.to_string(),
        only: None,
    };
    let wait = crate::wire::protocol::peer_timeout(shared.settings().heartbeat_interval);
    let link = match open_udp(ch, &connect.encode(), wait).await {
        Ok(link) => link,
        Err(Opening::Lost) => return Err(None),
        Err(Opening::Refused(status)) => return Err(Some(status)),
        Err(Opening::Silent) => {
            static SILENT: crate::policy::throttle::Throttle =
                crate::policy::throttle::Throttle::new();
            if let Some(more) = SILENT.allow() {
                warn!(
                    "service \"{name}\": client \"{}\" did not answer the UDP relay within {}s; trying again{more}",
                    session.name(),
                    wait.as_secs()
                );
            }
            if svc.clients.len() > 1 {
                shared.troubled(name, session, target, &crate::forward::no_reply());
            }
            return Err(None);
        }
    };
    if svc.clients.len() > 1 {
        shared.accepted(name, session.name(), target);
    }
    info!(
        "service \"{name}\": UDP relay ready via client \"{}\"",
        session.name()
    );

    let mut front = Front {
        socket,
        peers: Arc::new(Mutex::new(PeerTable::default())),
        gauge: PeerGauge::new(shared.clone()),
        meter: Arc::new(ServiceMeter {
            limit: shared.limit(name),
            stats: shared.metrics.service(name),
        }),
        stall: crate::wire::protocol::peer_timeout(shared.settings().heartbeat_interval),
    };
    let mut hooks = PublicPort {
        shared,
        name,
        svc,
        target,
        session,
        config,
        repick: tokio::time::interval(Duration::from_secs(1)),
        stop: shared.shutdown.clone(),
    };
    match front::run(&mut front, link, &mut hooks).await {
        Ending::Done => Ok(()),
        Ending::Broken => Err(None),
    }
}

struct ServiceMeter {
    limit: Arc<Limit>,
    stats: Arc<ServiceStats>,
}

impl Meter for ServiceMeter {
    fn take_in(&self, n: usize) -> bool {
        self.limit.inbound.try_take(n)
    }

    fn count_in(&self, n: usize) {
        self.stats
            .traffic
            .inbound
            .fetch_add(n as u64, Ordering::Relaxed);
    }

    fn new_peer(&self) {
        self.stats.total.fetch_add(1, Ordering::Relaxed);
    }

    fn take_out(&self, n: usize) -> bool {
        self.limit.outbound.try_take(n)
    }

    fn count_out(&self, n: usize) {
        self.stats
            .traffic
            .outbound
            .fetch_add(n as u64, Ordering::Relaxed);
    }
}

struct PublicPort<'a> {
    shared: &'a Arc<Shared>,
    name: &'a str,
    svc: &'a Service,
    target: &'a str,
    session: &'a super::via::Via,
    config: watch::Receiver<Service>,
    repick: tokio::time::Interval,
    stop: watch::Receiver<bool>,
}

impl Hooks for PublicPort<'_> {
    fn admit(&self, from: SocketAddr) -> bool {
        self.config.borrow().sources.permits(from.ip())
    }

    fn max_peers(&self) -> usize {
        self.shared.settings().udp_max_peers
    }

    fn timeout(&self) -> Duration {
        self.shared.settings().udp_session_timeout
    }

    fn full(&self, from: SocketAddr) {
        static FULL: crate::policy::throttle::Throttle = crate::policy::throttle::Throttle::new();
        if let Some(more) = FULL.allow() {
            warn!(
                "service \"{}\": udp.max_peers {} reached, dropping datagram from {from}; raise udp.max_peers in server.yaml{more}",
                self.name,
                self.shared.settings().udp_max_peers
            );
        }
    }

    fn stalled(&self, stall: Duration) {
        static STALLED: crate::policy::throttle::Throttle =
            crate::policy::throttle::Throttle::new();
        if let Some(more) = STALLED.allow() {
            warn!(
                "service \"{}\": client \"{}\" stopped reading its UDP relay for {}s; trying again{more}",
                self.name,
                self.session.name(),
                stall.as_secs()
            );
        }
        if self.svc.clients.len() > 1 {
            self.shared.troubled(
                self.name,
                self.session,
                self.target,
                &crate::forward::no_reply(),
            );
        }
    }

    fn recv_error(&self, e: &std::io::Error) {
        debug!("service \"{}\": UDP receive error: {e}", self.name);
    }

    async fn lost(&self) {
        self.session.closed().await
    }

    async fn interrupt(&mut self) -> Interrupt {
        loop {
            tokio::select! {
                _ = self.repick.tick() => {
                    let current = self.config.borrow().clone();
                    if let Some(preferred) = self
                        .shared
                        .preferred(self.name, &current, self.target)
                        .filter(|p| p != self.session.name())
                    {
                        debug!(
                            "service \"{}\": moving the UDP relay to client \"{preferred}\"",
                            self.name
                        );
                        return Interrupt::End(Ending::Done);
                    }
                }
                r = self.config.changed() => {
                    let moved = {
                        let next = self.config.borrow();
                        next.target != self.svc.target
                            || !next.clients.iter().any(|c| c == self.session.name())
                    };
                    if r.is_err() || moved {
                        return Interrupt::End(Ending::Done);
                    }
                    let sources = self.config.borrow().sources.clone();
                    return Interrupt::Retain(Box::new(move |addr| sources.permits(addr.ip())));
                }
                _ = self.session.closed() => return Interrupt::End(Ending::Done),
                _ = self.stop.changed() => return Interrupt::End(Ending::Done),
            }
        }
    }
}
