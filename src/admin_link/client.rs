use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use burrow_admin_proto::msg::{self, ErrorBody, Incoming, Request, Subscribe, Welcome};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::open;
use crate::base::failure::Failure;
use crate::config::settings::VisitorSettings;
use crate::dialer::Dialer;
use crate::dialer::admin::{AdminRx, AdminTx};

pub const ANSWER: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum LinkError {
    Open(Failure),
    Lost(String),
    Refused(ErrorBody),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Open(e) => match &e.hint {
                Some(h) => write!(f, "{} ({h})", e.message),
                None => f.write_str(&e.message),
            },
            LinkError::Lost(why) => f.write_str(why),
            LinkError::Refused(e) => write!(f, "{}: {}", e.code.as_str(), e.message),
        }
    }
}

struct Conn {
    rx: AdminRx,
    tx: AdminTx,
    heard: Instant,
}

struct Opened {
    conn: Conn,
    welcome: Welcome,
}

pub struct Link {
    settings: VisitorSettings,
    dialer: Dialer,
    client: &'static str,
    conn: Mutex<Option<Opened>>,
    next: AtomicU32,
}

enum Want<'a> {
    Reply(u32),
    Event(&'a str),
}

impl Link {
    pub fn new(settings: VisitorSettings, client: &'static str) -> Link {
        Link {
            dialer: Dialer::visitor(&settings),
            settings,
            client,
            conn: Mutex::new(None),
            next: AtomicU32::new(1),
        }
    }

    pub fn server(&self) -> &str {
        &self.settings.server
    }

    fn id(&self) -> u32 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    fn lost(&self) -> LinkError {
        LinkError::Lost(format!(
            "the admin connection to {} broke",
            self.settings.server
        ))
    }

    async fn connect(&self) -> Result<Opened, LinkError> {
        let (rx, tx) = open::open(&self.dialer, &self.settings, self.client)
            .await
            .map_err(LinkError::Open)?;
        let mut conn = Conn {
            rx,
            tx,
            heard: Instant::now(),
        };
        let id = self.id();
        let body = self
            .exchange(&mut conn, id, msg::req::WELCOME, Value::Null)
            .await?;
        let welcome = serde_json::from_value(body).map_err(|e| {
            LinkError::Lost(format!(
                "{} sent an unreadable welcome: {e}",
                self.settings.server
            ))
        })?;
        Ok(Opened { conn, welcome })
    }

    async fn ready<'a>(&self, slot: &'a mut Option<Opened>) -> Result<&'a mut Opened, LinkError> {
        let (_, silence) = crate::admin::transport::keepalive();
        if slot
            .as_ref()
            .is_some_and(|o| o.conn.heard.elapsed() >= silence / 2)
            && let Some(old) = slot.take()
        {
            old.conn.tx.close().await;
        }
        if slot.is_none() {
            *slot = Some(self.connect().await?);
        }
        slot.as_mut().ok_or_else(|| self.lost())
    }

    async fn send(&self, conn: &mut Conn, id: u32, t: &str, body: Value) -> Result<(), LinkError> {
        let req = Request {
            id,
            t: t.to_string(),
            body,
        };
        let text = serde_json::to_vec(&req).map_err(|e| LinkError::Lost(e.to_string()))?;
        conn.tx.send(&text).await.map_err(|_| self.lost())
    }

    async fn wait(&self, conn: &mut Conn, want: Want<'_>) -> Result<Value, LinkError> {
        let until = Instant::now() + ANSWER;
        loop {
            let got = tokio::time::timeout_at(until, conn.rx.recv())
                .await
                .map_err(|_| {
                    LinkError::Lost(format!("{} did not answer in time", self.settings.server))
                })?;
            let text = match got {
                Ok(Some(m)) => m,
                Ok(None) | Err(_) => return Err(self.lost()),
            };
            conn.heard = Instant::now();
            let Some(incoming) = std::str::from_utf8(&text).ok().and_then(Incoming::parse) else {
                continue;
            };
            match (incoming, &want) {
                (Incoming::Reply { re, result }, Want::Reply(id)) if re == *id => {
                    return result.map_err(LinkError::Refused);
                }
                (Incoming::Event { ev, body }, _) if ev == msg::ev::REVOKED => {
                    return Err(LinkError::Lost(revoked_reason(body)));
                }
                (Incoming::Event { ev, .. }, _) if ev == msg::ev::BYE => {
                    return Err(LinkError::Lost(format!(
                        "{} is stopping",
                        self.settings.server
                    )));
                }
                (Incoming::Event { ev, body }, Want::Event(e)) if ev == *e => return Ok(body),
                _ => continue,
            }
        }
    }

    async fn exchange(
        &self,
        conn: &mut Conn,
        id: u32,
        t: &str,
        body: Value,
    ) -> Result<Value, LinkError> {
        self.send(conn, id, t, body).await?;
        self.wait(conn, Want::Reply(id)).await
    }

    async fn drop_if_lost<T>(&self, slot: &mut Option<Opened>, out: &Result<T, LinkError>) {
        if matches!(out, Err(LinkError::Lost(_)))
            && let Some(old) = slot.take()
        {
            old.conn.tx.close().await;
        }
    }

    pub async fn welcome(&self) -> Result<Welcome, LinkError> {
        let mut slot = self.conn.lock().await;
        Ok(self.ready(&mut slot).await?.welcome.clone())
    }

    pub async fn request(&self, t: &str, body: Value) -> Result<Value, LinkError> {
        let mut slot = self.conn.lock().await;
        let id = self.id();
        let out = match self.ready(&mut slot).await {
            Ok(o) if !o.welcome.offers(t) => Err(LinkError::Refused(msg::older_server(t))),
            Ok(o) => self.exchange(&mut o.conn, id, t, body).await,
            Err(e) => Err(e),
        };
        self.drop_if_lost(&mut slot, &out).await;
        out
    }

    async fn subscribe(&self, conn: &mut Conn, status_ms: u64) -> Result<(), LinkError> {
        let sub = Subscribe {
            status_ms,
            config: false,
        };
        let body = serde_json::to_value(sub).map_err(|e| LinkError::Lost(e.to_string()))?;
        self.exchange(conn, self.id(), msg::req::SUBSCRIBE, body)
            .await
            .map(drop)
    }

    pub async fn status(&self) -> Result<Value, LinkError> {
        let mut slot = self.conn.lock().await;
        let out = match self.ready(&mut slot).await {
            Ok(o) => {
                let conn = &mut o.conn;
                let read = async {
                    self.subscribe(conn, msg::MIN_STATUS_MS).await?;
                    let status = self.wait(conn, Want::Event(msg::ev::STATUS)).await?;
                    self.subscribe(conn, 0).await?;
                    Ok(status)
                };
                read.await
            }
            Err(e) => Err(e),
        };
        self.drop_if_lost(&mut slot, &out).await;
        out
    }

    pub async fn close(&self) {
        if let Some(old) = self.conn.lock().await.take() {
            old.conn.tx.close().await;
        }
    }
}

fn revoked_reason(body: Value) -> String {
    serde_json::from_value::<msg::Revoked>(body)
        .map_or_else(|_| "access was revoked".to_string(), |r| r.reason)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_revoked_event_gives_its_reason_or_a_plain_one() {
        let sent = serde_json::to_value(burrow_admin_proto::msg::Revoked {
            reason: "admin removed".into(),
        })
        .unwrap();
        assert_eq!(super::revoked_reason(sent), "admin removed");
        let odd = serde_json::json!({"why": 1});
        assert_eq!(super::revoked_reason(odd), "access was revoked");
    }
}
