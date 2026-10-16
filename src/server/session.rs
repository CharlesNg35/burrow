use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot, watch};

use crate::handshake::Channel;
use crate::protocol::{KEEPALIVE_RECORD, ServerMsg, TOKEN_LEN};
use crate::secret::Secret;
use tokio::io::AsyncReadExt;

pub struct PoolConfig {
    pub min_idle: usize,
    pub max_idle: usize,
    pub max_pending: usize,
    pub keepalive: Duration,
}

struct Pool {
    idle: VecDeque<Parked>,
    waiters: VecDeque<oneshot::Sender<Channel>>,
    target: usize,
    requested: usize,
    last_request: Instant,
    last_starved: Instant,
    min_idle: usize,
    max_idle: usize,
    max_pending: usize,
    keepalive: Duration,
}

pub struct ClientSession {
    pub name: String,
    pub token: [u8; TOKEN_LEN],
    pub control: mpsc::Sender<ServerMsg>,
    pub active: AtomicUsize,
    pub secret: Secret,
    pool: Mutex<Pool>,
    closed: watch::Sender<bool>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TakeError {
    TooManyPending,
    Closed,
}

const REQUEST_RETRY: Duration = Duration::from_secs(5);
const DECAY_AFTER: Duration = Duration::from_secs(60);

impl ClientSession {
    pub fn new(
        name: String,
        token: [u8; TOKEN_LEN],
        control: mpsc::Sender<ServerMsg>,
        secret: Secret,
        cfg: &PoolConfig,
    ) -> Self {
        let now = Instant::now();
        ClientSession {
            name,
            token,
            control,
            active: AtomicUsize::new(0),
            secret,
            closed: watch::Sender::new(false),
            pool: Mutex::new(Pool {
                idle: VecDeque::new(),
                waiters: VecDeque::new(),
                target: cfg.min_idle,
                requested: 0,
                last_request: now,
                last_starved: now,
                min_idle: cfg.min_idle,
                max_idle: cfg.max_idle.max(1),
                max_pending: cfg.max_pending.max(1),
                keepalive: cfg.keepalive,
            }),
        }
    }

    pub fn reconfigure(&self, cfg: &PoolConfig) {
        let mut p = self.pool.lock().expect("pool lock");
        p.min_idle = cfg.min_idle;
        p.max_idle = cfg.max_idle.max(1);
        p.max_pending = cfg.max_pending.max(1);
        p.keepalive = cfg.keepalive;
        p.target = p.target.clamp(p.min_idle, p.max_idle);
        self.replenish(&mut p);
    }

    pub fn idle_count(&self) -> usize {
        self.pool.lock().expect("pool lock").idle.len()
    }

    pub fn waiter_count(&self) -> usize {
        self.pool.lock().expect("pool lock").waiters.len()
    }

    pub fn start(&self) {
        let mut p = self.pool.lock().expect("pool lock");
        self.replenish(&mut p);
    }

    pub fn offer(&self, mut ch: Channel) {
        let mut p = self.pool.lock().expect("pool lock");
        p.requested = p.requested.saturating_sub(1);
        while let Some(w) = p.waiters.pop_front() {
            match w.send(ch) {
                Ok(()) => {
                    self.replenish(&mut p);
                    return;
                }
                Err(back) => ch = back,
            }
        }
        if p.idle.len() < p.max_idle {
            let keepalive = p.keepalive;
            p.idle.push_back(Parked::new(ch, keepalive));
        }
    }

    pub async fn take(&self) -> Result<Channel, TakeError> {
        let rx = loop {
            let parked = {
                let mut p = self.pool.lock().expect("pool lock");
                match p.idle.pop_front() {
                    Some(parked) => parked,
                    None => {
                        p.waiters.retain(|w| !w.is_closed());
                        if p.waiters.len() >= p.max_pending {
                            return Err(TakeError::TooManyPending);
                        }
                        let (tx, rx) = oneshot::channel();
                        p.waiters.push_back(tx);
                        p.last_starved = Instant::now();
                        p.target = (p.target.max(1) * 2).min(p.max_idle);
                        self.replenish(&mut p);
                        break rx;
                    }
                }
            };
            if let Some(ch) = parked.claim().await {
                let mut p = self.pool.lock().expect("pool lock");
                self.replenish(&mut p);
                return Ok(ch);
            }
        };
        rx.await.map_err(|_| TakeError::Closed)
    }

    pub fn tick(&self) {
        let mut p = self.pool.lock().expect("pool lock");
        p.idle.retain(|parked| !parked.is_dead());
        p.waiters.retain(|w| !w.is_closed());
        if p.last_starved.elapsed() > DECAY_AFTER && p.target > p.min_idle {
            p.target -= 1;
            p.last_starved = Instant::now();
        }
        while p.idle.len() > p.target.max(p.min_idle) {
            p.idle.pop_back();
        }
        if p.last_request.elapsed() > REQUEST_RETRY {
            p.requested = 0;
        }
        self.replenish(&mut p);
    }

    fn replenish(&self, p: &mut Pool) {
        let waiting = p.waiters.iter().filter(|w| !w.is_closed()).count();
        let want = p.target.max(p.min_idle) + waiting;
        let have = p.idle.len() + p.requested;
        if want > have {
            let count = (want - have).min(u16::MAX as usize);
            if self
                .control
                .try_send(ServerMsg::OpenData {
                    count: count as u16,
                })
                .is_ok()
            {
                p.requested += count;
                p.last_request = Instant::now();
            }
        }
    }

    pub fn close(&self) {
        self.closed.send_replace(true);
        let mut p = self.pool.lock().expect("pool lock");
        p.idle.clear();
        p.waiters.clear();
    }

    pub async fn closed(&self) {
        let mut rx = self.closed.subscribe();
        let _ = rx.wait_for(|c| *c).await;
    }

    pub fn connection_guard(self: &Arc<Self>) -> (ActiveGuard, usize) {
        let before = self.active.fetch_add(1, Ordering::AcqRel);
        let guard = ActiveGuard {
            session: self.clone(),
        };
        (guard, before)
    }
}

struct Parked {
    release: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Option<Channel>>,
}

impl Parked {
    fn new(ch: Channel, keepalive: Duration) -> Parked {
        let (release, rx) = oneshot::channel();
        Parked {
            release,
            task: tokio::spawn(park(ch, rx, keepalive)),
        }
    }

    fn is_dead(&self) -> bool {
        self.task.is_finished()
    }

    async fn claim(self) -> Option<Channel> {
        self.release.send(()).ok()?;
        self.task.await.ok().flatten()
    }
}

enum ParkEvent {
    Release(bool),
    PeerActivity,
    Keepalive,
}

async fn park(
    mut ch: Channel,
    mut release: oneshot::Receiver<()>,
    interval: Duration,
) -> Option<Channel> {
    let keepalive = ch.stream.is_websocket();
    let mut probe = [0u8; 1];
    loop {
        let event = tokio::select! {
            r = &mut release => ParkEvent::Release(r.is_ok()),
            _ = ch.stream.read(&mut probe) => ParkEvent::PeerActivity,
            _ = tokio::time::sleep(interval), if keepalive => ParkEvent::Keepalive,
        };
        match event {
            ParkEvent::Release(true) => return Some(ch),
            ParkEvent::Release(false) | ParkEvent::PeerActivity => return None,
            ParkEvent::Keepalive => {
                if ch
                    .writer
                    .send(&mut ch.stream, &[KEEPALIVE_RECORD])
                    .await
                    .is_err()
                {
                    return None;
                }
            }
        }
    }
}

pub struct ActiveGuard {
    session: Arc<ClientSession>,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.session.active.fetch_sub(1, Ordering::Relaxed);
    }
}
