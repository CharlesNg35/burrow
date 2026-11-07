use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use burrow_admin_proto::local::Reason;
use burrow_admin_proto::msg::AdminLevel;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::time::Instant;

use super::journal::Journal;
use super::mode::{Mode, Writes};
use crate::admin_link::client::{Link, LinkError};

pub const WAIT: Duration = Duration::from_secs(30);
pub const HANDSHAKE_WAIT: Duration = Duration::from_secs(2);
pub const MAX_BACKOFF: Duration = Duration::from_secs(8);

#[derive(Debug, Clone)]
pub struct Problem {
    pub code: &'static str,
    pub message: String,
    pub hint: String,
}

#[derive(Debug, Clone)]
pub enum State {
    Connecting,
    Up(AdminLevel),
    Down {
        problem: Problem,
        failures: u32,
        at: Instant,
    },
}

pub struct Upstream {
    link: Arc<Link>,
    writes: Writes,
    state: watch::Sender<State>,
    reconnects: AtomicU32,
    ever_up: AtomicBool,
    journal: Option<Journal>,
}

pub fn backoff(failures: u32) -> Duration {
    let secs = 1u64 << failures.saturating_sub(1).min(8);
    Duration::from_secs(secs).min(MAX_BACKOFF)
}

pub fn problem(e: &LinkError, server: &str) -> Problem {
    const AGAIN: &str = "burrow mcp tries again on the next call";
    match e {
        LinkError::Open(Reason::Unreachable, f) => Problem {
            code: "no-connection",
            message: format!(
                "burrow mcp on this computer could not connect to {server}: {}",
                f.message
            ),
            hint: format!(
                "check this computer's network (Wi-Fi, VPN, DNS) and that the burrow server is running; {AGAIN}"
            ),
        },
        LinkError::Open(_, f) => Problem {
            code: "refused",
            message: format!(
                "burrow mcp on this computer reached the burrow server, which refused the admin session: {}",
                f.message
            ),
            hint: match &f.hint {
                Some(h) => format!("{h}; {AGAIN}"),
                None => AGAIN.to_string(),
            },
        },
        LinkError::Lost(why) => Problem {
            code: "connection-lost",
            message: format!("burrow mcp on this computer lost its link to {server}: {why}"),
            hint: format!(
                "it reconnects, with a fresh DNS lookup, on the next call; if this keeps happening, check this computer's network; {AGAIN}"
            ),
        },
        LinkError::Refused(b) => Problem {
            code: "refused",
            message: format!("{server} refused: {}", b.message),
            hint: AGAIN.to_string(),
        },
    }
}

impl Upstream {
    pub fn start(link: Arc<Link>, writes: Writes, journal: Option<Journal>) -> Arc<Upstream> {
        let up = Arc::new(Upstream {
            link,
            writes,
            state: watch::channel(State::Connecting).0,
            reconnects: AtomicU32::new(0),
            ever_up: AtomicBool::new(false),
            journal,
        });
        let first = up.clone();
        tokio::spawn(async move {
            let _ = first.attempt().await;
        });
        up
    }

    pub fn link(&self) -> &Link {
        &self.link
    }

    pub fn subscribe(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }

    pub fn mode(&self) -> Mode {
        match &*self.state.borrow() {
            State::Up(level) => Mode::of(*level, self.writes),
            _ => Mode::assumed(self.writes),
        }
    }

    pub fn is_up(&self) -> bool {
        matches!(&*self.state.borrow(), State::Up(_))
    }

    pub async fn settled(&self, within: Duration) {
        let mut rx = self.subscribe();
        let _ =
            tokio::time::timeout(within, rx.wait_for(|s| !matches!(s, State::Connecting))).await;
    }

    async fn attempt(&self) -> Result<(), LinkError> {
        let out = match tokio::time::timeout(WAIT, self.link.welcome()).await {
            Ok(Ok(w)) => Ok(w.you.level),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(LinkError::Lost(format!(
                "{} did not answer within {} s",
                self.link.server(),
                WAIT.as_secs()
            ))),
        };
        match out {
            Ok(level) => {
                self.up(level);
                Ok(())
            }
            Err(e) => {
                self.down(&e);
                Err(e)
            }
        }
    }

    fn up(&self, level: AdminLevel) {
        let mut was_down = false;
        self.state.send_modify(|s| {
            was_down = matches!(s, State::Down { .. });
            *s = State::Up(level);
        });
        if !self.ever_up.swap(true, Ordering::Relaxed) {
            self.note(&format!(
                "link to {} is up, as visitor \"{}\"",
                self.link.server(),
                self.link.visitor()
            ));
        } else if was_down {
            self.reconnects.fetch_add(1, Ordering::Relaxed);
            self.note(&format!("link to {} is up again", self.link.server()));
        }
    }

    fn down(&self, e: &LinkError) {
        let problem = problem(e, self.link.server());
        self.note(&format!("{} ({})", problem.message, problem.hint));
        self.state.send_modify(|s| {
            let failures = match s {
                State::Down { failures, .. } => *failures + 1,
                _ => 1,
            };
            *s = State::Down {
                problem,
                failures,
                at: Instant::now(),
            };
        });
    }

    fn note(&self, what: &str) {
        if let Some(j) = &self.journal {
            j.write(what);
        }
    }

    pub async fn ready(&self) -> Result<(), LinkError> {
        let mut rx = self.subscribe();
        let settled = tokio::time::timeout(WAIT, rx.wait_for(|s| !matches!(s, State::Connecting)))
            .await
            .ok()
            .and_then(Result::ok)
            .map(|s| s.clone());
        match settled {
            None => Err(LinkError::Lost(format!(
                "still connecting to {} after {} s",
                self.link.server(),
                WAIT.as_secs()
            ))),
            Some(State::Up(_)) => Ok(()),
            Some(State::Down { failures, at, .. }) => {
                tokio::time::sleep_until(at + backoff(failures)).await;
                self.attempt().await
            }
            Some(State::Connecting) => Ok(()),
        }
    }

    pub fn noted<T>(&self, r: &Result<T, LinkError>) {
        if let Err(e @ (LinkError::Open(..) | LinkError::Lost(_))) = r {
            self.down(e);
        }
    }

    pub fn describe(&self) -> Value {
        let mode = self.mode().word();
        let (state, error) = match &*self.state.borrow() {
            State::Connecting => ("connecting", None),
            State::Up(_) => ("up", None),
            State::Down { problem, .. } => ("down", Some(problem.message.clone())),
        };
        let mut v = json!({
            "state": state,
            "server": self.link.server(),
            "visitor": self.link.visitor(),
            "session": mode,
            "reconnects": self.reconnects.load(Ordering::Relaxed),
        });
        if let Some(e) = error {
            v["error"] = e.into();
        }
        v
    }

    pub fn failure(&self, e: &LinkError) -> Value {
        let mut v = match e {
            LinkError::Refused(b) => {
                json!({ "error": b.code.as_str(), "message": b.message, "details": b.body })
            }
            other => {
                let p = problem(other, self.link.server());
                json!({ "error": p.code, "message": p.message, "hint": p.hint })
            }
        };
        v["mcp_link"] = self.describe();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::failure::Failure;

    #[test]
    fn the_backoff_doubles_up_to_its_limit() {
        let waits: Vec<u64> = (1..=6).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 8, 8]);
        assert_eq!(backoff(0).as_secs(), 1);
        assert_eq!(backoff(u32::MAX), MAX_BACKOFF);
    }

    #[test]
    fn each_problem_says_where_it_failed_and_what_to_do() {
        let dns = LinkError::Open(
            Reason::Unreachable,
            Failure::new("could not look up tunnel.example.com (DNS: no such host)"),
        );
        let p = problem(&dns, "tunnel.example.com:7835");
        assert_eq!(
            p.message,
            "burrow mcp on this computer could not connect to tunnel.example.com:7835: could not look up tunnel.example.com (DNS: no such host)"
        );
        assert!(
            p.hint.starts_with("check this computer's network"),
            "{}",
            p.hint
        );
        assert!(
            p.hint.ends_with("tries again on the next call"),
            "{}",
            p.hint
        );
        let no_admin = LinkError::Open(
            Reason::Forbidden,
            Failure::new("visitor \"x\" has no admin rights on s").hint("add \"admin: write\""),
        );
        let p = problem(&no_admin, "s");
        assert!(
            p.message.contains("refused the admin session"),
            "{}",
            p.message
        );
        assert!(p.hint.starts_with("add \"admin: write\";"), "{}", p.hint);
        let lost = problem(
            &LinkError::Lost("the admin connection to s broke".into()),
            "s",
        );
        assert!(
            lost.message.contains("on this computer lost its link"),
            "{}",
            lost.message
        );
        assert!(lost.hint.contains("fresh DNS lookup"), "{}", lost.hint);
    }
}
