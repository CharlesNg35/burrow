mod channel;
mod watch;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use burrow_admin_proto::status::NotifyStatus;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use super::Shared;
use crate::config::settings::NotifyTarget;
use watch::{Message, Watch};

const TICK: Duration = Duration::from_secs(5);
const QUEUE: usize = 64;
const TRIES: u32 = 12;
const FIRST_RETRY: Duration = Duration::from_secs(5);
const LONGEST_RETRY: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub(crate) struct Letter {
    pub(crate) message: Message,
    pub(crate) server: Option<String>,
    pub(crate) at: u64,
}

impl Letter {
    pub(crate) fn title(&self) -> String {
        match &self.server {
            Some(s) => format!("[{s}] {}", self.message.title),
            None => format!("burrow: {}", self.message.title),
        }
    }
}

#[derive(Default)]
struct Board {
    sent: u64,
    dropped: u64,
    waiting: u64,
    failing: Option<(String, Instant)>,
}

impl Board {
    fn failed(&mut self, shown: &str, why: String) {
        if self.failing.is_none() {
            warn!("notifications to {shown} fail: {why}; burrow keeps trying");
            self.failing = Some((why, Instant::now()));
        } else if let Some((last, _)) = self.failing.as_mut() {
            *last = why;
        }
    }

    fn worked(&mut self, shown: &str) {
        if self.failing.take().is_some() {
            info!("notifications to {shown} work again");
        }
        self.sent += 1;
    }
}

struct Worker {
    target: NotifyTarget,
    tx: mpsc::Sender<Letter>,
    board: Arc<Mutex<Board>>,
    task: JoinHandle<()>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Worker {
    fn start(target: &NotifyTarget) -> Worker {
        let (tx, rx) = mpsc::channel(QUEUE);
        let board = Arc::new(Mutex::new(Board::default()));
        let sender = channel::open(&target.via);
        if let Ok(c) = &sender {
            debug!("notifications to {} go out as {}", target.shown(), c.kind());
        }
        let task = tokio::spawn(deliver(
            sender,
            rx,
            board.clone(),
            (target.shown(), FIRST_RETRY),
        ));
        Worker {
            target: target.clone(),
            tx,
            board,
            task,
        }
    }

    fn post(&self, letter: Letter) {
        let mut board = self.board.lock().expect("notify board lock");
        match self.tx.try_send(letter) {
            Ok(()) => board.waiting += 1,
            Err(_) => {
                board.dropped += 1;
                if board.dropped == 1 {
                    warn!(
                        "notifications to {} are {QUEUE} behind; newer ones are dropped until they catch up",
                        self.target.shown()
                    );
                }
            }
        }
    }

    fn status(&self) -> NotifyStatus {
        let b = self.board.lock().expect("notify board lock");
        NotifyStatus {
            name: self.target.shown(),
            sent: b.sent,
            waiting: b.waiting,
            dropped: b.dropped,
            failing: b.failing.as_ref().map(|(why, _)| why.clone()),
            failing_secs: b.failing.as_ref().map(|(_, at)| at.elapsed().as_secs()),
        }
    }
}

async fn deliver(
    sender: Result<Box<dyn channel::Channel>, channel::SendError>,
    mut rx: mpsc::Receiver<Letter>,
    board: Arc<Mutex<Board>>,
    (shown, first_retry): (String, Duration),
) {
    while let Some(letter) = rx.recv().await {
        let mut wait = first_retry;
        for attempt in 1..=TRIES {
            let result = match &sender {
                Ok(s) => s.send(&letter).await,
                Err(why) => Err(why.clone()),
            };
            let done = {
                let mut b = board.lock().expect("notify board lock");
                match result {
                    Ok(()) => {
                        b.worked(&shown);
                        true
                    }
                    Err(why) => {
                        let last = attempt == TRIES || !why.retryable();
                        b.failed(&shown, why.to_string());
                        if last {
                            b.dropped += 1;
                            debug!("gave up on a notification to {shown}: {}", letter.title());
                        }
                        last
                    }
                }
            };
            if done {
                break;
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(LONGEST_RETRY);
        }
        let mut b = board.lock().expect("notify board lock");
        b.waiting = b.waiting.saturating_sub(1);
    }
}

#[derive(Default)]
pub(crate) struct Notifier {
    workers: Mutex<Vec<Worker>>,
}

impl Notifier {
    pub(crate) fn sync(&self, targets: &[NotifyTarget]) {
        let mut workers = self.workers.lock().expect("notify workers lock");
        let mut old: Vec<Worker> = std::mem::take(&mut *workers);
        for t in targets {
            match old.iter().position(|w| w.target == *t) {
                Some(i) => workers.push(old.swap_remove(i)),
                None => workers.push(Worker::start(t)),
            }
        }
    }

    pub(crate) fn post(&self, letter: &Letter) {
        for w in self.workers.lock().expect("notify workers lock").iter() {
            if w.target.wants(letter.message.event) {
                w.post(letter.clone());
            }
        }
    }

    pub(crate) fn status(&self) -> Vec<NotifyStatus> {
        self.workers
            .lock()
            .expect("notify workers lock")
            .iter()
            .map(Worker::status)
            .collect()
    }
}

fn server_name(public_address: Option<&str>) -> Option<String> {
    let a = public_address?.trim();
    let a = a.split_once("://").map_or(a, |(_, rest)| rest);
    let host = a.split('/').next().unwrap_or(a);
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(v6),
        None => host.rsplit_once(':').map_or(host, |(h, _)| h),
    };
    (!host.is_empty()).then(|| host.to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub(super) async fn run(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut watch = Watch::default();
    loop {
        tick.tick().await;
        let settings = shared.settings();
        shared.notifier.sync(&settings.notify);
        if settings.notify.is_empty() {
            watch = Watch::default();
            continue;
        }
        let seen = watch::seen(&shared.status());
        let server = server_name(settings.public_address.as_deref());
        for message in watch.step(Instant::now(), seen) {
            info!("notify: {}", message.title);
            shared.notifier.post(&Letter {
                message,
                server: server.clone(),
                at: now_secs(),
            });
        }
    }
}

#[cfg(test)]
mod tests;
