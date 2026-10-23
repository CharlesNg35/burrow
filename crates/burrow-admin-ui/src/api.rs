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
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::Closure;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

use crate::store::{Conn, Store};

const TOKEN_KEY: &str = "burrow-admin-token";
const STATUS_MS: u64 = 1000;

type Reply = Result<Value, ErrorBody>;

struct Client {
    ws: WebSocket,
    ready: bool,
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
    if store.conn.get_untracked() == Conn::Ended {
        return;
    }
    let Some(token) = token() else {
        store.conn.set(Conn::NoToken);
        return;
    };
    let loc = window().location();
    let host = loc.host().unwrap_or_default();
    let prefix = crate::route::run_prefix();
    let Ok(ws) = WebSocket::new(&format!("ws://{host}{prefix}/api")) else {
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
    fail_pending();
    CLIENT.with(|c| {
        *c.borrow_mut() = Some(Client {
            ws,
            ready: false,
            next: 1,
            pending: HashMap::new(),
            _handlers: vec![Box::new(on_open), Box::new(on_message), Box::new(on_close)],
        })
    });
}

fn set_ready(ready: bool) {
    CLIENT.with(|c| {
        if let Some(c) = c.borrow_mut().as_mut() {
            c.ready = ready;
        }
    });
}

fn fail_pending() {
    let pending: Vec<Box<dyn FnOnce(Reply)>> = CLIENT.with(|c| {
        c.borrow_mut()
            .as_mut()
            .map(|c| {
                c.ready = false;
                c.pending.drain().map(|(_, f)| f).collect()
            })
            .unwrap_or_default()
    });
    for f in pending {
        f(Err(unanswered()));
    }
}

pub fn unanswered() -> ErrorBody {
    ErrorBody {
        code: ErrorCode::Unknown,
        message: "the connection closed before the server answered".into(),
        body: json!({ "unanswered": true }),
    }
}

fn closed(store: Store, e: &CloseEvent) {
    fail_pending();
    let was = store.conn.get_untracked();
    match was {
        Conn::Refused { .. } | Conn::Revoked { .. } | Conn::Closed { .. } | Conn::Ended => return,
        _ => {}
    }
    if e.code() == 1008 {
        store.conn.set(Conn::Ended);
        return;
    }
    probe(store);
    let attempt = ATTEMPT.with(|a| {
        let mut a = a.borrow_mut();
        *a += 1;
        *a
    });
    store.conn.set(Conn::Reconnecting { attempt });
    let delay = if store.admin_down.get_untracked() {
        2000
    } else {
        (1000 * u64::from(attempt.min(5))) as u32
    };
    store.retry_at.set(now_ms() + f64::from(delay));
    set_timeout(
        move || connect(store),
        std::time::Duration::from_millis(u64::from(delay)),
    );
}

fn probe(store: Store) {
    let url = format!("{}/api", crate::route::run_prefix());
    let answered: Closure<dyn FnMut(JsValue)> = Closure::once(move |r: JsValue| {
        let status = js_sys::Reflect::get(&r, &JsValue::from_str("status"))
            .ok()
            .and_then(|s| s.as_f64())
            .unwrap_or(0.0);
        let _ = store.admin_down.try_set(false);
        if status == 404.0 {
            let _ = store.conn.try_set(Conn::Ended);
        }
    });
    let unreachable: Closure<dyn FnMut(JsValue)> = Closure::once(move |_: JsValue| {
        let _ = store.admin_down.try_set(true);
    });
    let _ = window().fetch_with_str(&url).then2(&answered, &unreachable);
    answered.forget();
    unreachable.forget();
}

pub fn reconnect(store: Store) {
    ATTEMPT.with(|a| *a.borrow_mut() = 0);
    store.conn.set(Conn::Connecting);
    connect(store);
}

pub fn request(t: &str, body: Value, done: impl FnOnce(Reply) + 'static) {
    let mut done: Option<Box<dyn FnOnce(Reply)>> = Some(Box::new(done));
    CLIENT.with(|c| {
        let mut c = c.borrow_mut();
        let Some(c) = c
            .as_mut()
            .filter(|c| c.ready && c.ws.ready_state() == WebSocket::OPEN)
        else {
            return;
        };
        let id = c.next;
        c.next = c.next.wrapping_add(1).max(1);
        let text = json!({ "id": id, "t": t, "body": body }).to_string();
        if c.ws.send_with_str(&text).is_ok()
            && let Some(f) = done.take()
        {
            c.pending.insert(id, f);
        }
    });
    if let Some(f) = done {
        leptos::logging::warn!("burrow admin: request {t} not sent, no connection");
        set_timeout(
            move || {
                f(Err(ErrorBody {
                    code: ErrorCode::Unknown,
                    message: "not connected to the server; try again once the connection is back"
                        .into(),
                    body: Value::Null,
                }))
            },
            std::time::Duration::ZERO,
        );
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
                set_ready(true);
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
                    let busy = untrack(|| store.edit.dirty())
                        || store.edit.review.with_untracked(Option::is_some);
                    if busy {
                        let old = store.state.with_untracked(|s| {
                            s.as_ref().map(|s| s.rev.clone()).unwrap_or_default()
                        });
                        store.changed_from.set((old, js_sys::Date::now()));
                        store.changed.set(Some(c));
                    } else {
                        let by = if c.by.is_empty() || c.by == "file" {
                            "by a hand edit".to_string()
                        } else {
                            format!("by visitor {}", c.by)
                        };
                        store.toast(
                            burrow_ui_kit::Tone::Info,
                            format!("Config changed {by}; this page shows it now"),
                            None,
                        );
                    }
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
    fail_pending();
    if store.conn.with_untracked(Conn::is_final) {
        return;
    }
    let attempt = ATTEMPT.with(|a| *a.borrow());
    store.conn.set(Conn::Reconnecting {
        attempt: attempt + 1,
    });
}

fn after_refusal(now: &Conn, reason: String, message: String) -> Conn {
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
                if store.edit.sync_base(&s.values, &s.rev) {
                    store.edit.revalidate_now();
                }
                store.note_clients(&s);
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
