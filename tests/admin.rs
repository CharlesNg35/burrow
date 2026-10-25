mod common;
#[path = "admin/grants.rs"]
mod grants;
#[path = "admin/launch.rs"]
mod launch;
#[path = "admin/local_page.rs"]
mod local_page;
#[path = "admin/session.rs"]
mod session;

use std::time::Duration;

use burrow::dialer::Dialer;
use burrow::dialer::admin::{AdminOpenError, AdminRx, AdminTx};
use burrow::wire::secret::Secret;
use burrow_admin_proto::frame::AdminOpen;
use burrow_admin_proto::msg::{ErrorBody, Incoming};
use common::admin::{World, world, world_checked};
use serde_json::{Value, json};

struct Admin {
    rx: AdminRx,
    tx: AdminTx,
    next: u32,
    events: Vec<(String, Value)>,
}

fn open_msg(min: u8, max: u8) -> AdminOpen {
    AdminOpen {
        min,
        max,
        client: "tests/admin".into(),
        features: Vec::new(),
    }
}

async fn connect(
    w: &World,
    name: &str,
    secret: &Secret,
    min: u8,
    max: u8,
) -> Result<Admin, AdminOpenError> {
    let mut s = common::visitor_settings(w.addr, secret.clone(), Vec::new());
    s.name = name.to_string();
    let (rx, tx, _) = Dialer::visitor(&s).open_admin(&open_msg(min, max)).await?;
    Ok(Admin {
        rx,
        tx,
        next: 0,
        events: Vec::new(),
    })
}

impl Admin {
    async fn send_raw(&mut self, t: &str, body: Value) -> u32 {
        self.next += 1;
        let msg = json!({"id": self.next, "t": t, "body": body});
        self.tx.send(msg.to_string().as_bytes()).await.unwrap();
        self.next
    }

    async fn incoming(&mut self) -> Option<Incoming> {
        let m = tokio::time::timeout(Duration::from_secs(20), self.rx.recv())
            .await
            .expect("an answer in time")
            .ok()??;
        Some(Incoming::parse(std::str::from_utf8(&m).unwrap()).expect("a valid envelope"))
    }

    async fn call(&mut self, t: &str, body: Value) -> Result<Value, ErrorBody> {
        let id = self.send_raw(t, body).await;
        loop {
            match self.incoming().await.expect("the session stays open") {
                Incoming::Reply { re, result } if re == id => return result,
                Incoming::Reply { .. } => {}
                Incoming::Event { ev, body } => self.events.push((ev, body)),
            }
        }
    }

    async fn event(&mut self, name: &str) -> Value {
        if let Some(i) = self.events.iter().position(|(e, _)| e == name) {
            return self.events.remove(i).1;
        }
        loop {
            match self.incoming().await.expect("the session stays open") {
                Incoming::Event { ev, body } if ev == name => return body,
                Incoming::Event { ev, body } => self.events.push((ev, body)),
                Incoming::Reply { .. } => {}
            }
        }
    }

    async fn state(&mut self) -> Value {
        self.call("state", json!({})).await.unwrap()
    }

    async fn plan(&mut self, ops: Value) -> Result<Value, ErrorBody> {
        let rev = self.state().await["rev"].clone();
        self.call("validate", json!({"base": rev, "ops": ops}))
            .await
    }

    async fn apply(&mut self, ops: Value, confirm: &[&str]) -> Result<Value, ErrorBody> {
        let plan = self.plan(ops.clone()).await?;
        self.call(
            "apply",
            json!({"base": plan["base"], "ops": ops, "plan": plan["plan"], "confirm": confirm}),
        )
        .await
    }
}
