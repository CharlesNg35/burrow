use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use burrow_admin_proto::msg::{
    self, AuditEntry, ConfigEvent, ErrorBody, ErrorCode, HistoryReply, Incoming, Revoked,
    SchemaReply, State, Welcome,
};
use burrow_admin_proto::serde_json::{self, Value, json};
use burrow_admin_proto::status::Status;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

use crate::store::{Conn, Store};

const TOKEN_KEY: &str = "burrow-admin-token";
const STATUS_MS: u64 = 1000;

type Reply = Result<Value, ErrorBody>;

struct Client {
    ws: WebSocket,
    next: u32,
    pending: HashMap<u32, Box<dyn FnOnce(Reply)>>,
    _handlers: Vec<Box<dyn std::any::Any>>,
}

thread_local! {
    static CLIENT: RefCell<Option<Client>> = const { RefCell::new(None) };
    static ATTEMPT: RefCell<u32> = const { RefCell::new(0) };
}

fn now_ms() -> f64 {
    js_sys::Date::now()
}

fn token() -> Option<String> {
    let w = window();
    let storage = w.session_storage().ok().flatten();
    let hash = w.location().hash().unwrap_or_default();
    if let Some(t) = hash.strip_prefix("#token=").filter(|t| !t.is_empty()) {
        if let Some(s) = &storage {
            let _ = s.set_item(TOKEN_KEY, t);
        }
        let path = w.location().pathname().unwrap_or_else(|_| "/".into());
        if let Ok(h) = w.history() {
            let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&path));
        }
        return Some(t.to_string());
    }
    storage.and_then(|s| s.get_item(TOKEN_KEY).ok().flatten())
}

pub fn connect(store: Store) {
    let Some(token) = token() else {
        store.conn.set(Conn::NoToken);
        return;
    };
    let loc = window().location();
    let host = loc.host().unwrap_or_default();
    let Ok(ws) = WebSocket::new(&format!("ws://{host}/api")) else {
        store.conn.set(Conn::Closed {
            message: "The browser could not open a connection to burrow admin.".into(),
        });
        return;
    };
    let auth = json!({ "auth": token }).to_string();
    let ws_open = ws.clone();
    let on_open = Closure::<dyn FnMut()>::new(move || {
        let _ = ws_open.send_with_str(&auth);
    });
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(text) = e.data().as_string() {
            incoming(store, &text);
        }
    });
    let on_close = Closure::<dyn FnMut(CloseEvent)>::new(move |e: CloseEvent| closed(store, &e));
    ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    CLIENT.with(|c| {
        *c.borrow_mut() = Some(Client {
            ws,
            next: 1,
            pending: HashMap::new(),
            _handlers: vec![Box::new(on_open), Box::new(on_message), Box::new(on_close)],
        })
    });
}

fn fail_pending() {
    let pending: Vec<Box<dyn FnOnce(Reply)>> = CLIENT.with(|c| {
        c.borrow_mut()
            .as_mut()
            .map(|c| c.pending.drain().map(|(_, f)| f).collect())
            .unwrap_or_default()
    });
    for f in pending {
        f(Err(ErrorBody {
            code: ErrorCode::Unknown,
            message: "the connection closed".into(),
            body: Value::Null,
        }));
    }
}

fn closed(store: Store, e: &CloseEvent) {
    fail_pending();
    let was = store.conn.get_untracked();
    match was {
        Conn::Refused { .. } | Conn::Revoked { .. } | Conn::Closed { .. } => return,
        _ => {}
    }
    if e.code() == 1008 {
        store.conn.set(Conn::Closed {
            message:
                "burrow admin did not accept this page's sign-in. Open it again from the terminal."
                    .into(),
        });
        return;
    }
    let attempt = ATTEMPT.with(|a| {
        let mut a = a.borrow_mut();
        *a += 1;
        *a
    });
    store.conn.set(Conn::Reconnecting { attempt });
    let delay = (1000 * u64::from(attempt.min(5))) as u32;
    store.retry_at.set(now_ms() + f64::from(delay));
    set_timeout(
        move || connect(store),
        std::time::Duration::from_millis(u64::from(delay)),
    );
}

pub fn reconnect(store: Store) {
    ATTEMPT.with(|a| *a.borrow_mut() = 0);
    store.conn.set(Conn::Connecting);
    connect(store);
}

pub fn request(t: &str, body: Value, done: impl FnOnce(Reply) + 'static) {
    let sent = CLIENT.with(|c| {
        let mut c = c.borrow_mut();
        let c = c.as_mut()?;
        let id = c.next;
        c.next = c.next.wrapping_add(1).max(1);
        let text = json!({ "id": id, "t": t, "body": body }).to_string();
        c.ws.send_with_str(&text).ok()?;
        c.pending.insert(id, Box::new(done));
        Some(())
    });
    if sent.is_none() {
        leptos::logging::warn!("burrow admin: request {t} not sent, no connection");
    }
}

fn incoming(store: Store, text: &str) {
    match Incoming::parse(text) {
        Some(Incoming::Reply { re, result }) => {
            let f = CLIENT.with(|c| c.borrow_mut().as_mut().and_then(|c| c.pending.remove(&re)));
            if let Some(f) = f {
                f(result);
            }
        }
        Some(Incoming::Event { ev, body }) => event(store, &ev, body),
        None => leptos::logging::warn!("burrow admin: unreadable message"),
    }
}

fn str_of(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn event(store: Store, ev: &str, body: Value) {
    match ev {
        "bridge" => match str_of(&body, "state").as_str() {
            "open" => {
                ATTEMPT.with(|a| *a.borrow_mut() = 0);
                store.server_addr.set(str_of(&body, "server"));
                start(store);
            }
            "refused" => {
                let reason = str_of(&body, "reason");
                let message = str_of(&body, "message");
                store
                    .conn
                    .set(after_refusal(&store.conn.get_untracked(), reason, message));
            }
            _ => lost(store),
        },
        msg::ev::STATUS => {
            if let Ok(s) = serde_json::from_value::<Status>(body) {
                store.take_status(s, now_ms());
            }
        }
        msg::ev::CONFIG => {
            if let Ok(c) = serde_json::from_value::<ConfigEvent>(body) {
                let ours = store
                    .state
                    .with_untracked(|s| s.as_ref().is_some_and(|s| s.rev == c.rev))
                    || store.edit.outcome.with_untracked(
                        |o| matches!(o, Some(crate::edit::Outcome::Applied(a)) if a.rev == c.rev),
                    )
                    || store.edit.applying.get_untracked();
                if !ours {
                    let old = store
                        .state
                        .with_untracked(|s| s.as_ref().map(|s| s.rev.clone()).unwrap_or_default());
                    store.changed_from.set((old, js_sys::Date::now()));
                    store.changed.set(Some(c));
                    load_state(store);
                    load_history(store);
                }
            }
        }
        msg::ev::REVOKED => {
            let reason = serde_json::from_value::<Revoked>(body)
                .map(|r| r.reason)
                .unwrap_or_default();
            store.conn.set(Conn::Revoked { reason });
        }
        msg::ev::BYE => lost(store),
        _ => {}
    }
}

fn lost(store: Store) {
    if store.conn.with_untracked(Conn::is_final) {
        return;
    }
    let attempt = ATTEMPT.with(|a| *a.borrow());
    store.conn.set(Conn::Reconnecting {
        attempt: attempt + 1,
    });
}

pub fn after_refusal(now: &Conn, reason: String, message: String) -> Conn {
    match now {
        Conn::Revoked { .. } => now.clone(),
        Conn::Reconnecting { .. } if reason == "unreachable" || reason == "busy" => now.clone(),
        Conn::Reconnecting { .. } if reason == "forbidden" => Conn::Revoked { reason: message },
        _ => Conn::Refused { reason, message },
    }
}

fn start(store: Store) {
    request(msg::req::WELCOME, json!({}), move |r| match r {
        Ok(v) => match serde_json::from_value::<Welcome>(v) {
            Ok(w) => {
                let have = store
                    .schema
                    .with_untracked(|s| s.as_ref().map(|s| s.hash.clone()))
                    .filter(|h| *h == w.schema_hash)
                    .unwrap_or_default();
                store.welcome.set(Some(Arc::new(w)));
                load_schema(store, have);
            }
            Err(e) => store.error.set(Some(format!("unreadable welcome: {e}"))),
        },
        Err(e) => store.error.set(Some(e.message)),
    });
}

fn load_schema(store: Store, have: String) {
    request(msg::req::SCHEMA, json!({ "have": have }), move |r| match r
        .map(serde_json::from_value::<SchemaReply>)
    {
        Ok(Ok(reply)) => {
            if let Some(s) = reply.schema {
                store.schema.set(Some(Arc::new(s)));
            }
            load_state(store);
            request(
                msg::req::SUBSCRIBE,
                json!({ "status_ms": STATUS_MS, "config": true }),
                |_| {},
            );
            load_history(store);
        }
        Ok(Err(e)) => store.error.set(Some(format!("unreadable schema: {e}"))),
        Err(e) => store.error.set(Some(e.message)),
    });
}

pub fn load_state(store: Store) {
    request(msg::req::STATE, json!({}), move |r| {
        match r.map(serde_json::from_value::<State>) {
            Ok(Ok(s)) => {
                store.edit.sync_base(&s.values, &s.rev);
                store.state.set(Some(Arc::new(s)));
                if store.conn.get_untracked() != Conn::Open {
                    store.conn.set(Conn::Open);
                }
            }
            Ok(Err(e)) => store.error.set(Some(format!("unreadable state: {e}"))),
            Err(e) => store.error.set(Some(e.message)),
        }
    });
}

pub fn load_history(store: Store) {
    request(msg::req::HISTORY, json!({ "limit": 20 }), move |r| {
        if let Ok(Ok(h)) = r.map(serde_json::from_value::<HistoryReply>) {
            store.history.set(Some(h.entries));
        } else {
            store.history.set(Some(Vec::<AuditEntry>::new()));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_while_reconnecting_keep_trying_or_mean_revoked() {
        let r = Conn::Reconnecting { attempt: 2 };
        assert_eq!(after_refusal(&r, "unreachable".into(), "x".into()), r);
        assert!(
            matches!(after_refusal(&r, "forbidden".into(), "gone".into()), Conn::Revoked { reason } if reason == "gone")
        );
        let revoked = Conn::Revoked { reason: "x".into() };
        assert_eq!(
            after_refusal(&revoked, "forbidden".into(), "y".into()),
            revoked
        );
        assert!(matches!(
            after_refusal(&Conn::Connecting, "forbidden".into(), "y".into()),
            Conn::Refused { .. }
        ));
    }
}
