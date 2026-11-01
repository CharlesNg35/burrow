pub(crate) mod data;
#[cfg(unix)]
pub(crate) use data::fit_to_open_files;
pub mod reconnect;
pub mod reload;
mod session;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use crate::config::settings::ClientSettings;
use crate::dialer::Dialer;
use crate::wire::transport::resolve::Resolver;
use reconnect::{Backoff, Outage, Report, human};

const STABLE_AFTER: Duration = Duration::from_secs(10);
const REJECT_CONFIRM_ATTEMPTS: u32 = 3;
const MAX_OPENING: usize = 32;

pub(crate) struct Waiting(Arc<watch::Sender<usize>>);

impl Waiting {
    fn new(count: &Arc<watch::Sender<usize>>) -> Waiting {
        count.send_modify(|n| *n += 1);
        Waiting(count.clone())
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n -= 1);
    }
}
const DATA_PATIENCE: Duration = Duration::from_secs(5);
const LEAVE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Connecting,
    Connected,
    Rejected,
}

#[derive(Clone)]
struct Current {
    settings: Arc<ClientSettings>,
    dialer: Arc<Dialer>,
    dialing: Arc<data::Dialing>,
}

impl Current {
    fn new(settings: Arc<ClientSettings>, resolver: &Arc<dyn Resolver>) -> Current {
        Current {
            dialer: dialer_for(&settings, resolver),
            dialing: Arc::new(data::Dialing::of_client(&settings)),
            settings,
        }
    }
}

impl data::Dials for Current {
    fn dialing(&self) -> Arc<data::Dialing> {
        self.dialing.clone()
    }
}

#[derive(Clone)]
pub(crate) struct Live(Arc<watch::Sender<Current>>);

impl Live {
    pub(crate) fn settings(&self) -> Arc<ClientSettings> {
        self.0.borrow().settings.clone()
    }

    fn dialer(&self) -> Arc<Dialer> {
        self.0.borrow().dialer.clone()
    }
}

pub struct Client {
    live: Live,
    open: data::Open,
    resolver: Arc<dyn Resolver>,
    state: watch::Sender<State>,
    halt: watch::Sender<bool>,
    outage: Arc<Mutex<Outage>>,
    reloads: Option<mpsc::Receiver<ClientSettings>>,
    instance: [u8; crate::wire::protocol::TOKEN_LEN],
    welcomed: Arc<Mutex<Option<Instant>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    Stopped,
    GaveUp(Duration, String),
    Rejected,
}

pub enum SessionEnd {
    Lost(String),
    Rejected,
    Removed,
    Stop,
    Reconnect,
}

fn dialer_for(settings: &ClientSettings, resolver: &Arc<dyn Resolver>) -> Arc<Dialer> {
    let mut dialer = Dialer::client(settings);
    dialer.resolver = resolver.clone();
    Arc::new(dialer)
}

impl Client {
    pub fn new(settings: ClientSettings) -> Self {
        let (state, _) = watch::channel(State::Connecting);
        let resolver = crate::wire::transport::resolve::system();
        let open = data::Open::new(
            (
                data::client_connection_cap(settings.max_connections),
                data::client_peer_cap(settings.max_udp_peers),
            ),
            false,
        );
        let current = Current::new(Arc::new(settings), &resolver);
        Client {
            live: Live(Arc::new(watch::Sender::new(current))),
            open,
            resolver,
            state,
            halt: watch::Sender::new(false),
            outage: Arc::new(Mutex::new(Outage::default())),
            reloads: None,
            instance: {
                let mut id = [0u8; crate::wire::protocol::TOKEN_LEN];
                getrandom::fill(&mut id).expect("operating system random source is available");
                id
            },
            welcomed: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        let settings = self.live.settings();
        self.live.0.send_replace(Current::new(settings, &resolver));
        self.resolver = resolver;
        self
    }

    pub fn with_reloads(mut self, reloads: mpsc::Receiver<ClientSettings>) -> Self {
        self.reloads = Some(reloads);
        self
    }

    fn apply(&self, next: ClientSettings, backoff: &mut Backoff) -> bool {
        let old = self.live.settings();
        let changes = reload::compare(&old, &next);
        info!("{}", changes.describe());
        if changes.is_empty() {
            return false;
        }
        if (old.reconnect_initial, old.reconnect_max)
            != (next.reconnect_initial, next.reconnect_max)
        {
            *backoff = Backoff::new(next.reconnect_initial, next.reconnect_max);
        }
        if old.max_connections != next.max_connections {
            self.open
                .connections
                .limit(data::client_connection_cap(next.max_connections));
        }
        if old.max_udp_peers != next.max_udp_peers {
            self.open
                .peers
                .limit(data::client_peer_cap(next.max_udp_peers));
        }
        self.live
            .0
            .send_replace(Current::new(Arc::new(next), &self.resolver));
        !changes.reconnect.is_empty()
    }

    pub fn state(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }

    pub async fn run(mut self, mut stop: watch::Receiver<bool>) -> RunEnd {
        let mut reloads = self.reloads.take();
        let first = self.live.settings();
        if first.insecure_plaintext {
            warn!("insecure_plaintext is enabled: forwarded TCP data is NOT encrypted");
        }
        let mut backoff = Backoff::new(first.reconnect_initial, first.reconnect_max);
        let mut rejected: Option<(Instant, u32)> = None;
        let end = loop {
            let end = {
                let session = self.session(stop.clone());
                tokio::pin!(session);
                loop {
                    tokio::select! {
                        e = &mut session => break e,
                        next = crate::base::channel::next_reload(&mut reloads) => {
                            if self.apply(next, &mut backoff) {
                                break SessionEnd::Reconnect;
                            }
                        }
                    }
                }
            };
            let s = self.live.settings();
            let welcomed = self.welcomed.lock().expect("welcomed lock").take();
            if welcomed.is_some_and(|t| t.elapsed() > STABLE_AFTER) {
                backoff.reset();
            }
            let delay = backoff.next_delay();
            if !matches!(end, SessionEnd::Rejected) {
                rejected = None;
            }
            let why = match end {
                SessionEnd::Stop => break RunEnd::Stopped,
                SessionEnd::Reconnect => {
                    let _ = self.state.send(State::Connecting);
                    backoff.reset();
                    continue;
                }
                SessionEnd::Lost(why) => {
                    let _ = self.state.send(State::Connecting);
                    why
                }
                SessionEnd::Rejected => {
                    let now = Instant::now();
                    let (since, count) = rejected.get_or_insert((now, 0));
                    *count += 1;
                    let confirmed = *count >= REJECT_CONFIRM_ATTEMPTS
                        && now.duration_since(*since) >= s.reject_confirm;
                    if confirmed {
                        let _ = self.state.send(State::Rejected);
                    } else {
                        let _ = self.state.send(State::Connecting);
                    }
                    let why = format!(
                        "handshake rejected by {}: wrong secret or unknown client name \"{}\" (check both against the server config, or print a new client config on the server with: burrow client-config {} -o client.yaml)",
                        s.server, s.name, s.name
                    );
                    if confirmed && s.exit_on_rejected {
                        error!("{why}; not retrying (reconnect.on_rejected: exit)");
                        break RunEnd::Rejected;
                    }
                    why
                }
                SessionEnd::Removed => {
                    let _ = self.state.send(State::Rejected);
                    let why = format!("the server removed client \"{}\" from its config", s.name);
                    if s.exit_on_rejected {
                        error!("{why}; not retrying (reconnect.on_rejected: exit)");
                        break RunEnd::Rejected;
                    }
                    why
                }
            };
            let now = Instant::now();
            let report = self.outage.lock().expect("outage lock").failed(now);
            match report {
                Report::First => match s.give_up_after {
                    Some(limit) => warn!(
                        "{why}; retrying in {:.1}s, for up to {} (reconnect.give_up_after_secs)",
                        delay.as_secs_f64(),
                        human(limit)
                    ),
                    None => warn!(
                        "{why}; retrying in {:.1}s, and will keep retrying",
                        delay.as_secs_f64()
                    ),
                },
                Report::Summary { down_for, attempts } => warn!(
                    "still cannot reach {} after {} ({attempts} attempts); retrying every {}s at most. last error: {why}",
                    s.server,
                    human(down_for),
                    s.reconnect_max.as_secs()
                ),
                Report::Quiet => debug!("{why}; retrying in {:.1}s", delay.as_secs_f64()),
            }
            if let Some(limit) = s.give_up_after {
                let down = self.outage.lock().expect("outage lock").down_for(now);
                if down >= limit {
                    error!(
                        "giving up: no working connection to {} for {} (reconnect.give_up_after_secs: {}); last error: {why}",
                        s.server,
                        human(down),
                        limit.as_secs()
                    );
                    break RunEnd::GaveUp(down, why);
                }
            }
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    _ = stop.changed() => return self.stopped(),
                    next = crate::base::channel::next_reload(&mut reloads) => {
                        if self.apply(next, &mut backoff) {
                            rejected = None;
                            let _ = self.state.send(State::Connecting);
                            backoff.reset();
                            break;
                        }
                    }
                }
            }
        };
        if end == RunEnd::Stopped {
            return self.stopped();
        }
        self.halt.send_replace(true);
        end
    }

    fn stopped(&self) -> RunEnd {
        info!("client stopped");
        self.halt.send_replace(true);
        RunEnd::Stopped
    }
}
