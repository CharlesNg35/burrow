use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, watch};
use tracing::{info, warn};

use super::reconnect::{Backoff, human};
use super::{Client, DATA_PATIENCE, MAX_OPENING, SessionEnd, State, Waiting, data};
use crate::base::failure::io_reason;
use crate::dialer::DialError;
use crate::wire::protocol::{
    ByeReason, ChannelKind, ClientMsg, Hello, ServerMsg, ServiceInfo, TOKEN_LEN, VERSION,
};
use crate::wire::transport::Channel;

const STRAYS: usize = 3;

#[derive(Default)]
struct Strays {
    count: std::sync::atomic::AtomicUsize,
    seen: tokio::sync::Notify,
}

struct CloseOnDrop(Arc<Semaphore>, watch::Sender<bool>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
        self.1.send_replace(true);
    }
}

fn bye(reason: ByeReason) -> SessionEnd {
    match reason {
        ByeReason::Shutdown => SessionEnd::Lost("server is shutting down".into()),
        ByeReason::Replaced => SessionEnd::Lost(
            "another client connected with the same name and replaced this one".into(),
        ),
        ByeReason::Removed => SessionEnd::Removed,
        ByeReason::Timeout => SessionEnd::Lost("server timed out this connection".into()),
        ByeReason::Duplicate => SessionEnd::Lost(
            "the server already has a client with this name connected from elsewhere; is a copy of this config running on another machine? this one keeps trying and takes over when the other leaves"
                .into(),
        ),
        ByeReason::PlaintextMismatch => SessionEnd::Lost(
            "server and client disagree on insecure_plaintext (set it the same on both sides)"
                .into(),
        ),
    }
}

impl Client {
    pub(super) async fn open(
        &self,
        kind: ChannelKind,
        hello: Hello,
    ) -> Result<Channel, SessionEnd> {
        self.live
            .dialer()
            .open(kind, hello)
            .await
            .map_err(|e| match e {
                DialError::Lost(why) | DialError::Unreachable(why) => SessionEnd::Lost(why),
                DialError::Rejected => SessionEnd::Rejected,
            })
    }

    pub(super) async fn session(&self) -> SessionEnd {
        let s = self.live.settings();
        let ch = match self
            .open(
                ChannelKind::Control,
                Hello::Control {
                    version: VERSION,
                    instance: self.instance,
                },
            )
            .await
        {
            Ok(c) => c,
            Err(e) => return e,
        };
        let Channel {
            stream,
            mut writer,
            mut reader,
        } = ch;
        let (mut rd, mut wr) = stream.into_split();

        let (token, heartbeat) =
            match tokio::time::timeout(s.connect_timeout, reader.recv(&mut rd)).await {
                Ok(Ok(Some(b))) => match ServerMsg::decode(b) {
                    Ok(ServerMsg::Welcome {
                        token,
                        heartbeat_secs,
                    }) => (token, Duration::from_secs(heartbeat_secs.max(1).into())),
                    Ok(ServerMsg::Bye { reason }) => return bye(reason),
                    _ => return SessionEnd::Lost("server sent an unexpected first message".into()),
                },
                Ok(Ok(None)) => {
                    return SessionEnd::Lost(
                        "the server closed the connection before it was ready".into(),
                    );
                }
                Ok(Err(e)) => {
                    return SessionEnd::Lost(format!(
                        "connection to {} failed ({})",
                        s.server,
                        io_reason(&e)
                    ));
                }
                Err(_) => {
                    return SessionEnd::Lost(format!("server {} did not answer in time", s.server));
                }
            };
        *self.welcomed.lock().expect("welcomed lock") = Some(Instant::now());
        let _ = self.state.send(State::Connected);
        match self
            .outage
            .lock()
            .expect("outage lock")
            .recovered(Instant::now())
        {
            Some((down, attempts)) if attempts > 0 => info!(
                "connected to {} as \"{}\" (back after {}, {attempts} attempts)",
                s.server,
                s.name,
                human(down)
            ),
            _ => info!("connected to {} as \"{}\"", s.server, s.name),
        }

        let allow = ClientMsg::Allow(s.allow.describe());
        if writer.send(&mut wr, &allow.encode()).await.is_err() {
            return SessionEnd::Lost(format!("connection to {} lost", s.server));
        }
        let opening = Arc::new(Semaphore::new(MAX_OPENING));
        let waiting = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (ended, session_ended) = watch::channel(false);
        let _ended = CloseOnDrop(opening.clone(), ended);
        let strays = Arc::new(Strays::default());
        let timeout = heartbeat * 3;
        loop {
            let next = tokio::select! {
                r = tokio::time::timeout(timeout, reader.recv(&mut rd)) => r,
                _ = strays.seen.notified() => {
                    return SessionEnd::Lost(format!(
                        "{} does not know this session ({STRAYS} data channels were refused as stale), so its address may now lead to another server; reconnecting",
                        s.server
                    ));
                }
            };
            let msg = match next {
                Ok(Ok(Some(b))) => match ServerMsg::decode(b) {
                    Ok(m) => m,
                    Err(_) => return SessionEnd::Lost("server sent an invalid message".into()),
                },
                Ok(Ok(None)) => {
                    return SessionEnd::Lost(format!("server {} closed the connection", s.server));
                }
                Ok(Err(e)) => {
                    return SessionEnd::Lost(format!(
                        "connection to {} lost ({})",
                        s.server,
                        io_reason(&e)
                    ));
                }
                Err(_) => {
                    return SessionEnd::Lost(format!(
                        "no heartbeat from {} for {}s",
                        s.server,
                        timeout.as_secs()
                    ));
                }
            };
            match msg {
                ServerMsg::Ping => {
                    if writer
                        .send(&mut wr, &ClientMsg::Pong.encode())
                        .await
                        .is_err()
                    {
                        return SessionEnd::Lost(format!("connection to {} lost", s.server));
                    }
                }
                ServerMsg::OpenData { count } => {
                    let now = waiting.load(std::sync::atomic::Ordering::Acquire);
                    let most = data::WAITING_CAP.load(std::sync::atomic::Ordering::Relaxed);
                    let room = most.saturating_sub(now);
                    for _ in 0..usize::from(count).min(room) {
                        waiting.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        let held = Waiting(waiting.clone());
                        self.spawn_data(
                            token,
                            opening.clone(),
                            session_ended.clone(),
                            held,
                            strays.clone(),
                        );
                    }
                    if usize::from(count) > room {
                        static MANY: crate::policy::throttle::Throttle =
                            crate::policy::throttle::Throttle::new();
                        if let Some(more) = MANY.allow() {
                            warn!(
                                "the server asked for {count} more data channels while {now} are waiting; opening at most {most}{more}"
                            );
                        }
                    }
                }
                ServerMsg::Services(list) => self.report_services(&list),
                ServerMsg::Bye { reason } => return bye(reason),
                ServerMsg::Welcome { .. } => {}
            }
        }
    }

    fn report_services(&self, list: &[ServiceInfo]) {
        if list.is_empty() {
            info!("the server exposes no services for this client yet");
        }
        for svc in list {
            let ports = svc.ports.max(1);
            let at = match svc.bind_port {
                0 => "private, for visitors".to_string(),
                p if ports > 1 => format!(
                    "server ports {}",
                    crate::config::check::ports::show(p, ports)
                ),
                p => format!("server port {p}"),
            };
            let target = match crate::config::check::parse_host_port(&svc.target) {
                Ok((_, p)) if ports > 1 => format!(
                    "{}:{}",
                    svc.target.rsplit_once(':').map_or("", |(h, _)| h),
                    crate::config::check::ports::show(p, ports)
                ),
                _ => svc.target.clone(),
            };
            info!(
                "service \"{}\": {} {at} -> {target}",
                svc.name,
                svc.protocol.wire(),
            );
            data::precheck(&self.live.settings(), svc);
        }
    }

    fn spawn_data(
        &self,
        token: [u8; TOKEN_LEN],
        opening: Arc<Semaphore>,
        ended: watch::Receiver<bool>,
        held: Waiting,
        strays: Arc<Strays>,
    ) {
        let me = Client {
            live: self.live.clone(),
            open: self.open.clone(),
            resolver: self.resolver.clone(),
            state: self.state.clone(),
            halt: self.halt.clone(),
            outage: self.outage.clone(),
            reloads: None,
            instance: self.instance,
            welcomed: self.welcomed.clone(),
        };
        let mut halted = self.halt.subscribe();
        tokio::spawn(async move {
            tokio::select! {
                _ = me.data_channel(token, opening, ended, held, strays) => {}
                _ = halted.wait_for(|h| *h) => {}
            }
        });
    }

    async fn data_channel(
        &self,
        token: [u8; TOKEN_LEN],
        opening: Arc<Semaphore>,
        ended: watch::Receiver<bool>,
        held: Waiting,
        strays: Arc<Strays>,
    ) {
        let started = Instant::now();
        let mut backoff = Backoff::new(Duration::from_millis(100), Duration::from_secs(1));
        loop {
            let Ok(permit) = opening.clone().acquire_owned().await else {
                return;
            };
            let hello = Hello::Data {
                version: VERSION,
                token,
            };
            let ch = self.live.dialer().open(ChannelKind::Data, hello).await;
            drop(permit);
            match ch {
                Ok(ch) => {
                    let stale = data::serve(
                        data::Rules(self.live.0.clone(), self.open.clone()),
                        self.resolver.clone(),
                        ch,
                        ended,
                        held,
                    )
                    .await;
                    if !stale {
                        strays.count.store(0, std::sync::atomic::Ordering::Relaxed);
                    } else if strays
                        .count
                        .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                        + 1
                        >= STRAYS
                    {
                        strays.seen.notify_one();
                    }
                    return;
                }
                Err(DialError::Rejected | DialError::Lost(_))
                    if started.elapsed() < DATA_PATIENCE =>
                {
                    tokio::time::sleep(backoff.next_delay()).await;
                }
                Err(_) if *ended.borrow() => return,
                Err(DialError::Lost(why) | DialError::Unreachable(why)) => {
                    static FAILED: crate::policy::throttle::Throttle =
                        crate::policy::throttle::Throttle::new();
                    if let Some(more) = FAILED.allow() {
                        warn!("cannot open a data channel for the server: {why}{more}");
                    }
                    return;
                }
                Err(DialError::Rejected) => {
                    static REJECTED: crate::policy::throttle::Throttle =
                        crate::policy::throttle::Throttle::new();
                    if let Some(more) = REJECTED.allow() {
                        warn!(
                            "the server refused a data channel; connections through this client wait or fail until it reconnects{more}"
                        );
                    }
                    return;
                }
            }
        }
    }
}
