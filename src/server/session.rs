use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::time::Instant as Clock;

use burrow_syntax::share::{Share, Shares};
use tokio::sync::{mpsc, oneshot, watch};

use crate::wire::protocol::{KEEPALIVE_RECORD, ServerMsg, TOKEN_LEN};
use crate::wire::secret::Secret;
use crate::wire::transport::Channel;
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
    last_request: Clock,
    spare: usize,
    trim_at: Clock,
    min_idle: usize,
    max_idle: usize,
    max_pending: usize,
    keepalive: Duration,
}

pub struct ClientSession {
    pub name: String,
    pub peer: std::net::SocketAddr,
    pub since: Instant,
    pub token: [u8; TOKEN_LEN],
    pub control: mpsc::Sender<ServerMsg>,
    pub secret: Secret,
    pub instance: [u8; TOKEN_LEN],
    pub over_websocket: bool,
    pool: Mutex<Pool>,
    closed: watch::Sender<bool>,
    allow: Mutex<Option<Vec<String>>>,
    overflow: Shares<IpAddr>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TakeError {
    TooManyPending,
    Closed,
    TimedOut,
}

enum Queued {
    Within,
    Over { _share: Option<Share<IpAddr>> },
}

const WAIT_ROOM: usize = 4;
const REQUEST_RETRY: Duration = Duration::from_secs(5);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

impl ClientSession {
    pub fn new(
        name: String,
        peer: std::net::SocketAddr,
        token: [u8; TOKEN_LEN],
        control: mpsc::Sender<ServerMsg>,
        secret: Secret,
        instance: [u8; TOKEN_LEN],
        cfg: &PoolConfig,
    ) -> Self {
        let now = Clock::now();
        ClientSession {
            name,
            peer,
            since: Instant::now(),
            token,
            control,
            secret,
            instance,
            over_websocket: false,
            closed: watch::Sender::new(false),
            allow: Mutex::new(None),
            overflow: Shares::default(),
            pool: Mutex::new(Pool {
                idle: VecDeque::new(),
                waiters: VecDeque::new(),
                target: cfg.min_idle,
                requested: 0,
                last_request: now,
                spare: usize::MAX,
                trim_at: now,
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

    pub fn set_allow(&self, rules: Vec<String>) {
        *self.allow.lock().expect("allow lock") = Some(rules);
    }

    pub fn allow(&self) -> Option<Vec<String>> {
        self.allow.lock().expect("allow lock").clone()
    }

    pub fn idle_count(&self) -> usize {
        self.pool.lock().expect("pool lock").idle.len()
    }

    pub fn waiter_count(&self) -> usize {
        let p = self.pool.lock().expect("pool lock");
        p.waiters.iter().filter(|w| !w.is_closed()).count()
    }

    pub fn start(&self) {
        let mut p = self.pool.lock().expect("pool lock");
        self.replenish(&mut p);
    }

    pub fn offer(&self, mut ch: Channel) {
        let mut p = self.pool.lock().expect("pool lock");
        if *self.closed.borrow() {
            return;
        }
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

    pub async fn take(&self, wait: Duration, source: Option<IpAddr>) -> Result<Channel, TakeError> {
        let mut queued = Queued::Within;
        let took = tokio::time::timeout(wait, self.wait_for(source, &mut queued)).await;
        match (took, queued) {
            (Ok(r), _) => r,
            (Err(_), Queued::Within) => Err(TakeError::TimedOut),
            (Err(_), Queued::Over { .. }) => Err(TakeError::TooManyPending),
        }
    }

    async fn wait_for(
        &self,
        source: Option<IpAddr>,
        queued: &mut Queued,
    ) -> Result<Channel, TakeError> {
        let rx = loop {
            let parked = {
                let mut p = self.pool.lock().expect("pool lock");
                if *self.closed.borrow() {
                    return Err(TakeError::Closed);
                }
                let popped = p.idle.pop_front();
                p.spare = p.spare.min(p.idle.len());
                match popped {
                    Some(parked) => parked,
                    None => {
                        p.waiters.retain(|w| !w.is_closed());
                        *queued = self.queue(&p, source)?;
                        let (tx, rx) = oneshot::channel();
                        p.waiters.push_back(tx);
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

    fn queue(&self, p: &Pool, source: Option<IpAddr>) -> Result<Queued, TakeError> {
        let waiting = p.waiters.len();
        if waiting < p.max_pending {
            return Ok(Queued::Within);
        }
        let room = p.max_pending * (WAIT_ROOM - 1);
        let over = waiting - p.max_pending;
        if over >= room {
            return Err(TakeError::TooManyPending);
        }
        match source {
            None => Ok(Queued::Over { _share: None }),
            Some(ip) => self
                .overflow
                .take(ip, room, over)
                .map(|share| Queued::Over {
                    _share: Some(share),
                })
                .ok_or(TakeError::TooManyPending),
        }
    }

    pub fn tick(&self) {
        let mut p = self.pool.lock().expect("pool lock");
        p.idle.retain(|parked| !parked.is_dead());
        p.waiters.retain(|w| !w.is_closed());
        p.spare = p.spare.min(p.idle.len());
        let trim = p.trim_at.elapsed() >= IDLE_TIMEOUT;
        if trim {
            p.target = p.target.saturating_sub(p.spare).max(p.min_idle);
            p.trim_at = Clock::now();
        }
        while p.idle.len() > p.target.max(p.min_idle) {
            p.idle.pop_back();
        }
        if trim {
            p.spare = p.idle.len();
        }
        if p.last_request.elapsed() > REQUEST_RETRY {
            p.requested = 0;
        }
        self.replenish(&mut p);
    }

    fn replenish(&self, p: &mut Pool) {
        let waiting = p
            .waiters
            .iter()
            .filter(|w| !w.is_closed())
            .take(p.max_pending)
            .count();
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
                p.last_request = Clock::now();
            }
        }
    }

    pub fn close(&self) {
        self.closed.send_replace(true);
        let mut p = self.pool.lock().expect("pool lock");
        p.idle.clear();
        p.waiters.clear();
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    pub async fn closed(&self) {
        let mut rx = self.closed.subscribe();
        let _ = rx.wait_for(|c| *c).await;
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
    let mut probe = [0u8; 1];
    loop {
        let event = tokio::select! {
            r = &mut release => ParkEvent::Release(r.is_ok()),
            _ = ch.stream.read(&mut probe) => ParkEvent::PeerActivity,
            _ = tokio::time::sleep(interval) => ParkEvent::Keepalive,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const LONG: Duration = Duration::from_secs(30);

    fn session(max_pending: usize) -> Arc<ClientSession> {
        let (tx, rx) = mpsc::channel(4);
        std::mem::forget(rx);
        let cfg = PoolConfig {
            min_idle: 0,
            max_idle: 4,
            max_pending,
            keepalive: Duration::from_secs(25),
        };
        Arc::new(ClientSession::new(
            "home".into(),
            "127.0.0.1:1".parse().unwrap(),
            [0; TOKEN_LEN],
            tx,
            Secret::generate(),
            [0; TOKEN_LEN],
            &cfg,
        ))
    }

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    type Waiter = tokio::task::JoinHandle<Result<Channel, TakeError>>;

    async fn wait(s: &Arc<ClientSession>, source: Option<IpAddr>, n: usize) -> Vec<Waiter> {
        let goal = s.waiter_count() + n;
        let mut out = Vec::new();
        for _ in 0..n {
            let s = s.clone();
            out.push(tokio::spawn(async move { s.take(LONG, source).await }));
        }
        while s.waiter_count() < goal {
            tokio::task::yield_now().await;
        }
        out
    }

    async fn refused_at_once(s: &ClientSession, source: Option<IpAddr>) -> bool {
        let took = tokio::time::timeout(Duration::from_secs(1), s.take(LONG, source)).await;
        matches!(took, Ok(Err(TakeError::TooManyPending)))
    }

    fn pooled(min_idle: usize, idle: usize) -> (Arc<ClientSession>, Vec<Channel>) {
        let (tx, rx) = mpsc::channel(1024);
        std::mem::forget(rx);
        let cfg = PoolConfig {
            min_idle,
            max_idle: 64,
            max_pending: 64,
            keepalive: Duration::from_secs(25),
        };
        let s = Arc::new(ClientSession::new(
            "home".into(),
            "127.0.0.1:1".parse().unwrap(),
            [0; TOKEN_LEN],
            tx,
            Secret::generate(),
            [0; TOKEN_LEN],
            &cfg,
        ));
        s.pool.lock().unwrap().target = idle;
        let mut far = Vec::new();
        for _ in 0..idle {
            let (near, other) = crate::wire::transport::local::pair();
            s.offer(near);
            far.push(other);
        }
        (s, far)
    }

    async fn after(s: &ClientSession, secs: u64) {
        for _ in 0..secs / 5 {
            tokio::time::advance(Duration::from_secs(5)).await;
            s.tick();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn channels_no_take_needed_for_a_whole_minute_close_down_to_min_idle() {
        let (s, _far) = pooled(4, 64);
        assert_eq!(s.idle_count(), 64);
        after(&s, 30).await;
        assert_eq!(s.idle_count(), 64, "a channel stays for a full minute");
        after(&s, 40).await;
        assert_eq!(s.idle_count(), 4);
        assert_eq!(s.pool.lock().unwrap().target, 4);
    }

    #[tokio::test(start_paused = true)]
    async fn a_pool_in_use_keeps_the_channels_its_takes_reach() {
        let (s, mut far) = pooled(4, 16);
        for _ in 0..36 {
            let mut taken = Vec::new();
            for _ in 0..12 {
                taken.push(s.take(LONG, None).await.unwrap());
            }
            drop(taken);
            for _ in 0..12 {
                let (near, other) = crate::wire::transport::local::pair();
                s.offer(near);
                far.push(other);
            }
            after(&s, 5).await;
        }
        let p = s.pool.lock().unwrap();
        assert_eq!(
            p.target, 12,
            "the 4 never taken in a minute go, the 12 in use stay"
        );
    }

    #[tokio::test]
    async fn a_closed_session_answers_take_at_once() {
        let s = session(4);
        s.close();
        let took = tokio::time::timeout(Duration::from_secs(1), s.take(LONG, None)).await;
        assert!(matches!(took, Ok(Err(TakeError::Closed))));
        assert_eq!(s.waiter_count(), 0);
    }

    #[tokio::test]
    async fn a_burst_past_max_pending_waits_and_is_served_in_order() {
        let s = session(2);
        let waiters = wait(&s, None, 8).await;
        for _ in 0..8 {
            s.offer(crate::wire::transport::local::pair().0);
        }
        for w in waiters {
            assert!(w.await.unwrap().is_ok());
        }
        assert_eq!(s.waiter_count(), 0);
    }

    #[tokio::test]
    async fn the_waiting_room_is_four_times_max_pending_then_busy_at_once() {
        let s = session(2);
        let waiters = wait(&s, None, 2 * WAIT_ROOM).await;
        assert!(refused_at_once(&s, None).await);
        s.offer(crate::wire::transport::local::pair().0);
        while s.waiter_count() == 2 * WAIT_ROOM {
            tokio::task::yield_now().await;
        }
        let one_more = wait(&s, None, 1).await;
        assert!(refused_at_once(&s, None).await);
        s.close();
        for w in waiters.into_iter().chain(one_more) {
            let r = w.await.unwrap();
            assert!(matches!(r, Ok(_) | Err(TakeError::Closed)));
        }
    }

    #[tokio::test]
    async fn a_wait_past_max_pending_ends_busy_at_its_deadline_and_within_it_times_out() {
        let s = session(1);
        let within = {
            let s = s.clone();
            tokio::spawn(async move { s.take(Duration::from_millis(300), None).await })
        };
        while s.waiter_count() < 1 {
            tokio::task::yield_now().await;
        }
        let over = s.take(Duration::from_millis(300), ip("203.0.113.7")).await;
        assert!(matches!(over, Err(TakeError::TooManyPending)));
        assert!(matches!(within.await.unwrap(), Err(TakeError::TimedOut)));
        assert_eq!(s.waiter_count(), 0);
        assert!(s.overflow.is_empty(), "every share is given back");
    }

    #[tokio::test]
    async fn one_address_holds_a_fair_part_of_the_waiting_room() {
        let s = session(16);
        let a = ip("203.0.113.7");
        let held = wait(&s, a, 16 + 25).await;
        assert!(refused_at_once(&s, a).await, "a is over its part");
        let b = wait(&s, ip("198.51.100.1"), 8).await;
        assert!(refused_at_once(&s, ip("198.51.100.1")).await);
        let local = wait(&s, None, 64 - 49).await;
        assert!(refused_at_once(&s, None).await, "the room is full");
        s.close();
        for w in held.into_iter().chain(b).chain(local) {
            assert!(matches!(w.await.unwrap(), Err(TakeError::Closed)));
        }
        assert!(s.overflow.is_empty());
    }

    #[tokio::test]
    async fn closing_the_session_ends_every_waiter_at_once() {
        let s = session(1);
        let waiters = wait(&s, ip("203.0.113.7"), 4).await;
        s.close();
        for w in waiters {
            let r = tokio::time::timeout(Duration::from_secs(1), w).await;
            assert!(matches!(r, Ok(Ok(Err(TakeError::Closed)))));
        }
        assert!(s.overflow.is_empty());
    }

    #[tokio::test]
    async fn a_smaller_max_pending_on_reload_keeps_the_waiters_and_shrinks_the_room() {
        let s = session(4);
        let waiters = wait(&s, None, 8).await;
        s.reconfigure(&PoolConfig {
            min_idle: 0,
            max_idle: 4,
            max_pending: 1,
            keepalive: Duration::from_secs(25),
        });
        assert!(refused_at_once(&s, None).await);
        for _ in 0..8 {
            s.offer(crate::wire::transport::local::pair().0);
        }
        for w in waiters {
            assert!(w.await.unwrap().is_ok());
        }
    }
}
