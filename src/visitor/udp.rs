use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, warn};

use super::Live;
use super::forward::{explain, rejected};
use crate::client::reconnect::Backoff;
use crate::dialer::DialError;
use crate::policy::throttle::Throttle;
use crate::udp::front::{self, Ending, Front, Hooks, Interrupt, Meter};
use crate::udp::link::{Opening, open_udp};
use crate::udp::peers::{PeerCount, PeerGauge, PeerTable};
use crate::wire::protocol::{
    ChannelKind, Hello, MAX_DATAGRAM, VERSION, VisitRequest, VisitTarget, encode_datagram,
};
use burrow_config::Protocol;

const IDLE_CHECK: Duration = Duration::from_secs(1);
const STEADY: Duration = Duration::from_secs(1);

pub(super) struct Count(pub Arc<AtomicUsize>);

impl PeerCount for Count {
    fn peers(&self) -> &AtomicUsize {
        &self.0
    }
}

struct Unmetered;

impl Meter for Unmetered {
    fn take_in(&self, _: usize) -> bool {
        true
    }
    fn count_in(&self, _: usize) {}
    fn new_peer(&self) {}
    fn take_out(&self, _: usize) -> bool {
        true
    }
    fn count_out(&self, _: usize) {}
}

struct Port<'a> {
    name: &'a str,
    live: watch::Receiver<Live>,
    peers: front::Peers,
    idle: tokio::time::Interval,
}

impl Hooks for Port<'_> {
    fn admit(&self, _: SocketAddr) -> bool {
        true
    }

    fn max_peers(&self) -> usize {
        self.live.borrow().settings.udp_max_peers
    }

    fn timeout(&self) -> Duration {
        self.live.borrow().settings.udp_session_timeout
    }

    fn full(&self, from: SocketAddr) {
        static FULL: Throttle = Throttle::new();
        if let Some(more) = FULL.allow() {
            warn!(
                "forward \"{}\": udp.max_peers {} reached, dropping a datagram from {from}; raise udp.max_peers in visitor.yaml{more}",
                self.name,
                self.max_peers()
            );
        }
    }

    fn stalled(&self, stall: Duration) {
        static STALLED: Throttle = Throttle::new();
        if let Some(more) = STALLED.allow() {
            warn!(
                "forward \"{}\": the server stopped reading UDP for {}s; reopening on the next datagram{more}",
                self.name,
                stall.as_secs()
            );
        }
    }

    fn recv_error(&self, e: &std::io::Error) {
        debug!("forward \"{}\": UDP receive error: {e}", self.name);
    }

    async fn lost(&self) {
        std::future::pending::<()>().await
    }

    async fn interrupt(&mut self) -> Interrupt {
        loop {
            self.idle.tick().await;
            if self.peers.lock().expect("peers lock").is_empty() {
                return Interrupt::End(Ending::Done);
            }
        }
    }
}

pub(super) async fn serve(
    name: String,
    to: VisitTarget,
    socket: UdpSocket,
    live: watch::Receiver<Live>,
    count: Arc<AtomicUsize>,
) {
    let Ok(request) = (VisitRequest {
        check: false,
        protocol: Protocol::Udp,
        to: to.clone(),
    })
    .encode() else {
        warn!("forward \"{name}\": {to} is too long to send");
        return;
    };
    let peers: front::Peers = Arc::new(Mutex::new(PeerTable::default()));
    let mut front = Front {
        socket: Arc::new(socket),
        peers: peers.clone(),
        gauge: PeerGauge::new(Count(count)),
        meter: Arc::new(Unmetered),
        stall: crate::forward::VISIT_REPLY,
    };
    let mut backoff = Backoff::new(Duration::from_millis(100), Duration::from_secs(5));
    let mut sweep = tokio::time::interval(front::SWEEP);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let waiting = !peers.lock().expect("peers lock").is_empty();
        tokio::select! {
            r = front.socket.readable() => {
                if let Err(e) = r {
                    debug!("forward \"{name}\": UDP receive error: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            }
            _ = sweep.tick(), if waiting => {
                let timeout = live.borrow().settings.udp_session_timeout;
                let left = {
                    let mut p = peers.lock().expect("peers lock");
                    p.expire(timeout);
                    p.len()
                };
                front.gauge.set(left);
                continue;
            }
        }
        let (first, from) = match receive_from(&front.socket, &name) {
            Received::Datagram(data, from) => (data, from),
            Received::Nothing => continue,
            Received::Failed => {
                tokio::time::sleep(RECEIVE_ERROR_PAUSE).await;
                continue;
            }
        };
        if first.len() > MAX_DATAGRAM {
            front::too_large(from, first.len());
            continue;
        }
        let (id, count) = {
            let settings = live.borrow().settings.clone();
            let mut p = peers.lock().expect("peers lock");
            p.expire(settings.udp_session_timeout);
            (p.id_for(from, || settings.udp_max_peers), p.len())
        };
        front.gauge.set(count);
        let Some(id) = id else {
            continue;
        };
        let dialer = live.borrow().dialer.clone();
        let opened = match dialer
            .open(ChannelKind::Visit, Hello::Visit { version: VERSION })
            .await
        {
            Ok(ch) => open_udp(ch, &request, crate::forward::VISIT_REPLY).await,
            Err(e) => {
                static LOST: Throttle = Throttle::new();
                if let Some(more) = LOST.allow() {
                    let why = match e {
                        DialError::Rejected => rejected(&dialer),
                        DialError::Lost(why) | DialError::Unreachable(why) => why,
                    };
                    warn!("forward \"{name}\": {why}{more}");
                }
                tokio::time::sleep(backoff.next_delay()).await;
                continue;
            }
        };
        let mut link = match opened {
            Ok(link) => link,
            Err(failed) => {
                static REFUSED: Throttle = Throttle::new();
                if let Some(more) = REFUSED.allow() {
                    let why = match failed {
                        Opening::Refused(status) => {
                            explain(status, &to, &dialer.name, Protocol::Udp)
                        }
                        Opening::Silent => "the server did not answer the UDP visit in time".into(),
                        Opening::Lost => "the server closed the UDP visit before answering".into(),
                    };
                    warn!("forward \"{name}\": {why}{more}");
                }
                tokio::time::sleep(backoff.next_delay()).await;
                continue;
            }
        };
        let mut rec = Vec::with_capacity(4 + first.len());
        encode_datagram(id, &first, &mut rec);
        if link.writer.send(&mut link.wr, &rec).await.is_err() {
            tokio::time::sleep(backoff.next_delay()).await;
            continue;
        }
        let started = tokio::time::Instant::now();
        debug!("forward \"{name}\": UDP visit open");
        let mut hooks = Port {
            name: &name,
            live: live.clone(),
            peers: peers.clone(),
            idle: tokio::time::interval(IDLE_CHECK),
        };
        match front::run(&mut front, link, &mut hooks).await {
            Ending::Done => debug!("forward \"{name}\": UDP visit closed, no peers left"),
            Ending::Broken => {
                debug!("forward \"{name}\": UDP visit ended; the next datagram opens a new one")
            }
        }
        if started.elapsed() < STEADY {
            tokio::time::sleep(backoff.next_delay()).await;
        } else {
            backoff.reset();
        }
    }
}

thread_local! {
    static RECEIVE: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0u8; 65536]);
}

pub(super) enum Received {
    Datagram(Vec<u8>, SocketAddr),
    Nothing,
    Failed,
}

pub(super) fn receive_from(socket: &UdpSocket, name: &str) -> Received {
    RECEIVE.with(|buf| {
        let mut buf = buf.borrow_mut();
        match socket.try_recv_from(&mut buf) {
            Ok((n, from)) => Received::Datagram(buf[..n].to_vec(), from),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Received::Nothing,
            Err(e) => {
                debug!("forward \"{name}\": UDP receive error: {e}");
                Received::Failed
            }
        }
    })
}

pub(super) const RECEIVE_ERROR_PAUSE: Duration = Duration::from_millis(100);
