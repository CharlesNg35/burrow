use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tracing::{debug, info, warn};

use super::socks::{self, Asked, Reply};
use crate::client::reconnect::Backoff;
use crate::config::settings::VisitorSettings;
use crate::dialer::{DialError, Dialer};
use crate::forward::{ForwardError, pump_tcp, request_visit};
use crate::protocol::{ChannelKind, Hello, Status, VERSION, VisitRequest, VisitTarget};
use crate::throttle::Throttle;
use crate::transport::tcp::tune;
use burrow_config::Protocol;

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

    async fn wait(&self, patience: Duration) -> Result<tokio::sync::OwnedSemaphorePermit, bool> {
        let acquire = self.free.clone().acquire_owned();
        tokio::pin!(acquire);
        let deadline = tokio::time::Instant::now() + patience * 4;
        loop {
            let seen = self.opened.load(std::sync::atomic::Ordering::Relaxed);
            let window =
                patience.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
            match tokio::time::timeout(window, &mut acquire).await {
                Ok(permit) => return permit.map_err(|_| false),
                Err(_) if self.opened.load(std::sync::atomic::Ordering::Relaxed) != seen => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(true);
                    }
                }
                Err(_) => return Err(false),
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
            Err(e) if crate::transport::tcp::gone_before_accept(&e) => {
                debug!("forward \"{name}\": a connection was gone before it was accepted: {e}");
                continue;
            }
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

pub(super) fn rejected(dialer: &Dialer) -> String {
    format!(
        "the server {} refused the handshake: wrong secret or unknown visitor name \"{}\" (check the name and secret against the server config, or print a new visitor config on the server with: burrow visitor-config {} -o visitor.yaml)",
        dialer.server, dialer.name, dialer.name
    )
}

pub(super) fn explain(
    status: Status,
    to: &VisitTarget,
    visitor: &str,
    protocol: Protocol,
) -> String {
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
        (Status::Invalid, VisitTarget::Service { name, .. }) if protocol == Protocol::Udp => {
            format!(
                "the server refused UDP for service \"{name}\": it is a tcp service there (remove \"protocol: udp\" from this forward), or the server is older than v0.1.2, which carries UDP for visitors"
            )
        }
        (Status::Invalid, VisitTarget::Service { name, .. }) => format!(
            "the server refused service \"{name}\" as invalid: if it is a udp service there, add \"protocol: udp\" to this forward"
        ),
        (Status::Invalid, _) if protocol == Protocol::Udp => format!(
            "the server refused UDP to {goal}: the server is older than v0.1.2, which carries UDP for visitors, or the target is not a valid host:port"
        ),
        (Status::Invalid, _) => format!("the request for {goal} was rejected as invalid"),
        (Status::Ok, _) => String::new(),
    }
}

fn told_app(reply: Reply) -> String {
    format!(
        " (the SOCKS app was told: {}, code {})",
        reply.meaning(),
        reply as u8
    )
}

pub(super) fn socks_target_ok(target: &str) -> bool {
    target.len() <= crate::config::check::MAX_TARGET_LEN
        && !target.starts_with(crate::config::check::unix::PREFIX)
        && crate::config::check::parse_host_port(target).is_ok()
}

fn not_ready(why: &str) -> String {
    let until = if why.ends_with("(DNS)") {
        "it resolves"
    } else {
        "the server is reachable"
    };
    format!("{why}; the forwards will work once {until}")
}

pub(super) async fn check(dialer: Arc<Dialer>, forwards: Vec<(String, VisitTarget, Protocol)>) {
    let mut ready = true;
    for (name, to, protocol) in forwards {
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
                warn!("{}", not_ready(&why));
                return;
            }
        };
        let request = VisitRequest {
            check: true,
            protocol,
            to,
        };
        match ask(ch, &request).await {
            Some(Status::Ok) => {}
            Some(Status::Invalid) => {
                ready = false;
                let why = match crate::init::fetch::ask(&dialer).await {
                    Ok(fetched) => mismatch(&fetched, &request.to, protocol),
                    Err(_) => None,
                };
                warn!(
                    "forward \"{name}\": {}",
                    why.unwrap_or_else(|| explain(
                        Status::Invalid,
                        &request.to,
                        &dialer.name,
                        protocol
                    ))
                );
            }
            Some(st) => {
                ready = false;
                warn!(
                    "forward \"{name}\": {}",
                    explain(st, &request.to, &dialer.name, protocol)
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

fn mismatch(
    fetched: &crate::init::fetch::Fetched,
    to: &VisitTarget,
    protocol: Protocol,
) -> Option<String> {
    if fetched.tcp_only && protocol == Protocol::Udp {
        return Some(
            "the server is older than v0.1.2, which carries UDP for visitors (update it with burrow update, or remove this forward), or it lists more services than fit in one answer and this service is not a udp service there".into(),
        );
    }
    let VisitTarget::Service { name, .. } = to else {
        return None;
    };
    let service = fetched.grants.services.iter().find(|s| &s.name == name)?;
    match (service.protocol, protocol) {
        (Protocol::Udp, Protocol::Tcp) => Some(format!(
            "service \"{name}\" is a udp service on the server; add \"protocol: udp\" to this forward in visitor.yaml (or pass --udp)"
        )),
        (Protocol::Tcp, Protocol::Udp) => Some(format!(
            "service \"{name}\" is a tcp service on the server; remove \"protocol: udp\" from this forward in visitor.yaml (or leave out --udp)"
        )),
        _ => None,
    }
}

async fn ask(mut ch: crate::transport::Channel, request: &VisitRequest) -> Option<Status> {
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
    live: tokio::sync::watch::Receiver<super::Live>,
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
                Ok(Ok(Asked::Connect(target))) if socks_target_ok(&target) => {
                    VisitTarget::Client { client, target }
                }
                Ok(Ok(Asked::Associate(hint))) => {
                    super::socks_udp::associate(super::socks_udp::Association {
                        name,
                        client,
                        stream,
                        peer,
                        hint,
                        live,
                    })
                    .await;
                    return;
                }
                Ok(Ok(Asked::Connect(target))) => {
                    debug!(%peer, "forward \"{name}\": SOCKS target {target:?} is not a valid host:port");
                    let _ = socks::reply(&mut stream, Reply::HostUnreachable).await;
                    return;
                }
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::Unsupported => {
                    static ASKED: Throttle = Throttle::new();
                    if let Some(more) = ASKED.allow() {
                        info!(%peer, "forward \"{name}\": SOCKS request refused: {e}{more}");
                    }
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
        protocol: Protocol::Tcp,
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
        let patience = OPEN_PATIENCE + settings.connect_timeout;
        let permit = match opening.wait(patience).await {
            Ok(permit) => permit,
            Err(queued) => {
                if socks {
                    let _ = socks::reply(&mut stream, Reply::NetworkUnreachable).await;
                }
                static BUSY: Throttle = Throttle::new();
                if let Some(more) = BUSY.allow() {
                    if queued {
                        warn!(
                            "forward \"{name}\": closing a connection that waited {}s in line: new connections arrive faster than the server accepts them{more}",
                            (patience * 4).as_secs()
                        );
                    } else {
                        warn!(
                            "forward \"{name}\": closing a connection that waited {}s without the server answering any dial{more}",
                            patience.as_secs()
                        );
                    }
                }
                return;
            }
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
    let mut told = None;
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
                told = Some(code);
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
                    "forward \"{name}\": {}{}{more}",
                    explain(st, &to, &dialer.name, Protocol::Tcp),
                    told.map(told_app).unwrap_or_default()
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

    #[test]
    fn the_first_check_says_what_the_forwards_wait_for() {
        assert_eq!(
            not_ready("could not look up tunnel.example.com within 10 s (DNS)"),
            "could not look up tunnel.example.com within 10 s (DNS); the forwards will work once it resolves"
        );
        assert_eq!(
            not_ready("server unreachable at tunnel.example.com:7835 (connection refused)"),
            "server unreachable at tunnel.example.com:7835 (connection refused); the forwards will work once the server is reachable"
        );
    }

    #[test]
    fn an_invalid_answer_names_the_protocol_fix_or_an_old_server() {
        let dns = VisitTarget::service("dns");
        let udp = explain(Status::Invalid, &dns, "laptop", Protocol::Udp);
        assert!(udp.contains("remove \"protocol: udp\""), "{udp}");
        assert!(udp.contains("older than v0.1.2"), "{udp}");
        let tcp = explain(Status::Invalid, &dns, "laptop", Protocol::Tcp);
        assert!(tcp.contains("add \"protocol: udp\""), "{tcp}");
        let reach = VisitTarget::Client {
            client: "home".into(),
            target: "10.0.0.1:53".into(),
        };
        let reach = explain(Status::Invalid, &reach, "laptop", Protocol::Udp);
        assert!(reach.contains("older than v0.1.2"), "{reach}");
    }

    #[test]
    fn a_protocol_mismatch_names_the_fix_and_an_old_server_says_so() {
        use crate::init::fetch::Fetched;
        use crate::protocol::{GrantedService, Grants};
        let fetched = |protocol, tcp_only| Fetched {
            grants: Grants {
                services: vec![GrantedService {
                    name: "dns".into(),
                    protocol,
                    port: 53,
                    ports: 1,
                }],
                clients: vec![],
            },
            tcp_only,
        };
        let dns = VisitTarget::service("dns");
        let add = mismatch(&fetched(Protocol::Udp, false), &dns, Protocol::Tcp).unwrap();
        assert!(add.contains("add \"protocol: udp\""), "{add}");
        assert!(add.ends_with("(or pass --udp)"), "{add}");
        let remove = mismatch(&fetched(Protocol::Tcp, false), &dns, Protocol::Udp).unwrap();
        assert!(remove.contains("remove \"protocol: udp\""), "{remove}");
        assert!(remove.ends_with("(or leave out --udp)"), "{remove}");
        assert_eq!(
            mismatch(&fetched(Protocol::Udp, false), &dns, Protocol::Udp),
            None
        );
        let old = mismatch(&fetched(Protocol::Tcp, true), &dns, Protocol::Udp).unwrap();
        assert!(old.contains("older than v0.1.2"), "{old}");
        assert!(
            old.contains("more services than fit in one answer"),
            "{old}"
        );
    }

    #[test]
    fn a_socks_refusal_names_the_reply_the_app_got() {
        assert_eq!(
            told_app(Reply::from(Status::Denied)),
            " (the SOCKS app was told: connection not allowed by ruleset, code 2)"
        );
    }

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
        assert!(held.is_ok());
        assert_eq!(
            slots.wait(Duration::from_millis(300)).await.err(),
            Some(false)
        );
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
        assert_eq!(
            slots.wait(Duration::from_millis(200)).await.err(),
            Some(true)
        );
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
        let (_live, live) = tokio::sync::watch::channel(super::super::Live {
            dialer: dialer.clone(),
            settings: settings.clone(),
        });
        tokio::time::timeout(
            OPEN_PATIENCE + settings.connect_timeout + Duration::from_secs(3),
            visit(dialer, settings, full, conn, live),
        )
        .await
        .expect("the connection gives up waiting");
        let mut buf = [0u8; 1];
        assert_eq!(local.read(&mut buf).await.unwrap(), 0);
    }
}
