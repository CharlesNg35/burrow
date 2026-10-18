use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tracing::{debug, info, warn};

use super::socks::{self, Reply};
use crate::client::reconnect::Backoff;
use crate::config::settings::VisitorSettings;
use crate::dialer::{DialError, Dialer};
use crate::forward::{ForwardError, pump_tcp, request_visit};
use crate::protocol::{ChannelKind, Hello, Status, VERSION, VisitRequest, VisitTarget};
use crate::throttle::Throttle;
use crate::transport::tcp::tune;

const OPEN_PATIENCE: Duration = Duration::from_secs(5);
const SOCKS_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct Slots {
    free: Arc<Semaphore>,
    opened: std::sync::atomic::AtomicU64,
}

impl Slots {
    pub(super) fn new(count: usize) -> Slots {
        Slots {
            free: Arc::new(Semaphore::new(count)),
            opened: std::sync::atomic::AtomicU64::new(0),
        }
    }

    async fn wait(&self, patience: Duration) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let acquire = self.free.clone().acquire_owned();
        tokio::pin!(acquire);
        let deadline = tokio::time::Instant::now() + patience * 4;
        loop {
            let seen = self.opened.load(std::sync::atomic::Ordering::Relaxed);
            let window =
                patience.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
            match tokio::time::timeout(window, &mut acquire).await {
                Ok(permit) => return permit.ok(),
                Err(_)
                    if self.opened.load(std::sync::atomic::Ordering::Relaxed) != seen
                        && tokio::time::Instant::now() < deadline => {}
                Err(_) => return None,
            }
        }
    }
}

pub(super) struct Incoming {
    name: String,
    to: VisitTarget,
    socks: bool,
    stream: TcpStream,
    peer: SocketAddr,
}

pub(super) async fn accept(
    name: String,
    to: VisitTarget,
    socks: bool,
    listener: TcpListener,
    tx: mpsc::Sender<Incoming>,
) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                static FAILED: Throttle = Throttle::new();
                if let Some(more) = FAILED.allow() {
                    warn!("forward \"{name}\": accept failed: {e}{more}");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let conn = Incoming {
            name: name.clone(),
            to: to.clone(),
            socks,
            stream,
            peer,
        };
        if tx.send(conn).await.is_err() {
            return;
        }
    }
}

fn rejected(dialer: &Dialer) -> String {
    format!(
        "the server {} closed the connection during the handshake: wrong secret or unknown visitor name \"{}\", or the server is busy (check the name and secret against the server config, or print a new visitor config on the server with: burrow visitor-config {} -o visitor.yaml)",
        dialer.server, dialer.name, dialer.name
    )
}

fn explain(status: Status, to: &VisitTarget, visitor: &str) -> String {
    let goal = match to {
        VisitTarget::Service { name, .. } => format!("service \"{name}\""),
        VisitTarget::Client { target, .. } => target.clone(),
    };
    match (status, to) {
        (Status::Forbidden, VisitTarget::Service { name, .. }) => format!(
            "the server does not let visitor \"{visitor}\" use service \"{name}\": it does not exist, or does not list \"{visitor}\" in its \"visitors\""
        ),
        (Status::Forbidden, VisitTarget::Client { client, target }) if !target.is_empty() => {
            format!(
                "the server does not let visitor \"{visitor}\" reach {target} on client \"{client}\": its \"reach\" for \"{client}\" in the server config must include it, for example {client}: any"
            )
        }
        (Status::Forbidden, VisitTarget::Client { client, .. }) => format!(
            "the server does not let visitor \"{visitor}\" reach client \"{client}\" directly: that needs \"reach: {{ {client}: any }}\" for \"{visitor}\" in the server config"
        ),
        (Status::Offline, VisitTarget::Service { name, .. }) => format!(
            "the client behind service \"{name}\" is not connected to the server, or did not answer in time (see the client's log)"
        ),
        (Status::Offline, VisitTarget::Client { client, .. }) => format!(
            "client \"{client}\" is not connected to the server, or did not answer in time (see the client's log)"
        ),
        (Status::Busy, _) => {
            "the server is at a connection limit for this visitor, service or client".into()
        }
        (Status::Denied, VisitTarget::Client { .. }) => format!(
            "the client refused {goal}: it is not in the client's allow list, or not in the reach the server gives visitor \"{visitor}\""
        ),
        (Status::Denied, _) => {
            format!("the client refused {goal}: its target is not in the client's allow list")
        }
        (Status::Unreachable, _) => {
            format!("the client could not connect to {goal} (is it running?)")
        }
        (Status::Unresolved, _) => format!("the client could not resolve the host of {goal}"),
        (Status::Invalid, VisitTarget::Service { name, index }) if *index > 0 => format!(
            "the server refused port {} of service \"{name}\": the service has fewer ports than this forward's local range, or its client rejected the target",
            index + 1
        ),
        (Status::Invalid, _) => format!("the request for {goal} was rejected as invalid"),
        (Status::Ok, _) => String::new(),
    }
}

fn socks_target_ok(target: &str) -> bool {
    target.len() <= crate::config::check::MAX_TARGET_LEN
        && !target.starts_with(crate::config::check::unix::PREFIX)
        && crate::config::check::parse_host_port(target).is_ok()
}

pub(super) async fn check(dialer: Arc<Dialer>, forwards: Vec<(String, VisitTarget)>) {
    let mut ready = true;
    for (name, to) in forwards {
        let ch = match dialer
            .open(ChannelKind::Visit, Hello::Visit { version: VERSION })
            .await
        {
            Ok(ch) => ch,
            Err(DialError::Rejected) => {
                warn!("{}", rejected(&dialer));
                return;
            }
            Err(DialError::Lost(why) | DialError::Unreachable(why)) => {
                warn!("{why}; the forwards will work once the server is reachable");
                return;
            }
        };
        let request = VisitRequest { check: true, to };
        match ask(ch, &request).await {
            Some(Status::Ok) => {}
            Some(st) => {
                ready = false;
                warn!(
                    "forward \"{name}\": {}",
                    explain(st, &request.to, &dialer.name)
                );
            }
            None => {
                ready = false;
                warn!("forward \"{name}\": the server did not answer the check");
            }
        }
    }
    if ready {
        info!(
            "the server at {} accepts visitor \"{}\"; forwards are ready",
            dialer.server, dialer.name
        );
    }
}

async fn ask(mut ch: crate::handshake::Channel, request: &VisitRequest) -> Option<Status> {
    ch.writer
        .send(&mut ch.stream, &request.encode().ok()?)
        .await
        .ok()?;
    let answer = tokio::time::timeout(Duration::from_secs(10), ch.reader.recv(&mut ch.stream));
    match answer.await {
        Ok(Ok(Some(b))) => Status::decode(b).ok(),
        _ => None,
    }
}

pub(super) async fn visit(
    dialer: Arc<Dialer>,
    settings: Arc<VisitorSettings>,
    opening: Arc<Slots>,
    conn: Incoming,
) {
    let Incoming {
        name,
        to,
        socks,
        mut stream,
        peer,
    } = conn;
    tune(&stream, &settings.tcp);
    let to = match (socks, to) {
        (true, VisitTarget::Client { client, .. }) => {
            match tokio::time::timeout(SOCKS_TIMEOUT, socks::request(&mut stream)).await {
                Ok(Ok(target)) if socks_target_ok(&target) => {
                    VisitTarget::Client { client, target }
                }
                Ok(Ok(target)) => {
                    debug!(%peer, "forward \"{name}\": SOCKS target {target:?} is not a valid host:port");
                    let _ = socks::reply(&mut stream, Reply::HostUnreachable).await;
                    return;
                }
                Ok(Err(e)) => {
                    debug!(%peer, "forward \"{name}\": SOCKS request refused: {e}");
                    return;
                }
                Err(_) => {
                    debug!(%peer, "forward \"{name}\": no SOCKS request in time");
                    return;
                }
            }
        }
        (_, to) => to,
    };
    let Ok(request) = (VisitRequest {
        check: false,
        to: to.clone(),
    })
    .encode() else {
        if socks {
            let _ = socks::reply(&mut stream, Reply::HostUnreachable).await;
        }
        debug!(%peer, "forward \"{name}\": {to} is too long to send");
        return;
    };
    let started = tokio::time::Instant::now();
    let mut backoff = Backoff::new(Duration::from_millis(100), Duration::from_secs(1));
    let answer = loop {
        let Some(permit) = opening.wait(OPEN_PATIENCE + settings.connect_timeout).await else {
            if socks {
                let _ = socks::reply(&mut stream, Reply::NetworkUnreachable).await;
            }
            static BUSY: Throttle = Throttle::new();
            if let Some(more) = BUSY.allow() {
                warn!(
                    "forward \"{name}\": closing a connection that waited {}s without the server answering any dial{more}",
                    (OPEN_PATIENCE + settings.connect_timeout).as_secs()
                );
            }
            return;
        };
        let ch = dialer
            .open(ChannelKind::Visit, Hello::Visit { version: VERSION })
            .await;
        drop(permit);
        if ch.is_ok() {
            opening
                .opened
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let patient = started.elapsed() < OPEN_PATIENCE;
        match ch {
            Ok(mut ch) => match request_visit(&mut ch, &request).await {
                Ok(()) => break Ok(ch),
                Err(ForwardError::Io(_)) if patient => {}
                Err(e) => break Err(e),
            },
            Err(DialError::Rejected | DialError::Lost(_)) if patient => {}
            Err(e) => {
                if socks {
                    let _ = socks::reply(&mut stream, Reply::NetworkUnreachable).await;
                }
                static LOST: Throttle = Throttle::new();
                if let Some(more) = LOST.allow() {
                    let why = match e {
                        DialError::Rejected => rejected(&dialer),
                        DialError::Lost(why) | DialError::Unreachable(why) => why,
                    };
                    warn!("forward \"{name}\": {why}{more}");
                }
                return;
            }
        }
        tokio::time::sleep(backoff.next_delay()).await;
    };
    let result = match answer {
        Ok(ch) if socks => match socks::reply(&mut stream, Reply::Succeeded).await {
            Ok(()) => pump_tcp(ch, stream, false).await,
            Err(e) => Err(ForwardError::Io(e)),
        },
        Ok(ch) => pump_tcp(ch, stream, false).await,
        Err(e) => {
            if socks {
                let code = match &e {
                    ForwardError::Status(st) => Reply::from(*st),
                    _ => Reply::Failure,
                };
                let _ = socks::reply(&mut stream, code).await;
            }
            Err(e)
        }
    };
    match result {
        Ok(()) => debug!(%peer, "forward \"{name}\": connection closed"),
        Err(ForwardError::Status(st)) => {
            static REFUSED: Throttle = Throttle::new();
            if let Some(more) = REFUSED.allow() {
                warn!(
                    "forward \"{name}\": {}{more}",
                    explain(st, &to, &dialer.name)
                );
            }
        }
        Err(ForwardError::Protocol) => {
            warn!("forward \"{name}\": the server sent an invalid reply")
        }
        Err(ForwardError::Io(e)) => debug!(%peer, "forward \"{name}\": connection ended: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_slot_wait_goes_on_while_other_dials_succeed() {
        let slots = Arc::new(Slots::new(0));
        let busy = slots.clone();
        tokio::spawn(async move {
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                busy.opened
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            busy.free.add_permits(1);
        });
        let held = slots.wait(Duration::from_millis(300)).await;
        assert!(held.is_some());
        assert!(slots.wait(Duration::from_millis(300)).await.is_none());
    }

    #[tokio::test]
    async fn a_slot_wait_ends_even_while_other_dials_keep_succeeding() {
        let slots = Arc::new(Slots::new(0));
        let busy = slots.clone();
        let progress = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                busy.opened
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        });
        let started = std::time::Instant::now();
        assert!(slots.wait(Duration::from_millis(200)).await.is_none());
        assert!(started.elapsed() < Duration::from_millis(1500));
        progress.abort();
    }

    #[tokio::test]
    async fn a_connection_does_not_wait_forever_for_a_free_dial_slot() {
        use tokio::io::AsyncReadExt;
        let settings = Arc::new(super::super::tests::settings());
        let dialer = Arc::new(Dialer::visitor(&settings));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut local = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (stream, peer) = listener.accept().await.unwrap();
        let conn = Incoming {
            name: "db".into(),
            to: VisitTarget::service("db"),
            socks: false,
            stream,
            peer,
        };
        let full = Arc::new(Slots::new(0));
        tokio::time::timeout(
            OPEN_PATIENCE + settings.connect_timeout + Duration::from_secs(3),
            visit(dialer, settings, full, conn),
        )
        .await
        .expect("the connection gives up waiting");
        let mut buf = [0u8; 1];
        assert_eq!(local.read(&mut buf).await.unwrap(), 0);
    }
}
