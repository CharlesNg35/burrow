use burrow_config::ServerConfig;
use std::sync::Arc;

use burrow_admin_logic::rates::{Rates, Recorder, rates};
use burrow_admin_proto::msg::{AuditEntry, ConfigEvent, State, Welcome};
use burrow_admin_proto::schema::Schema;
use burrow_admin_proto::status::Status;
use burrow_ui_kit::{Toast, Tone};
use leptos::prelude::*;

use crate::edit::Edit;
use crate::route::Route;

#[derive(Debug, Clone, PartialEq)]
pub enum Conn {
    NoToken,
    Connecting,
    Open,
    Reconnecting { attempt: u32 },
    Refused { reason: String, message: String },
    Closed { message: String },
    Revoked { reason: String },
}

impl Conn {
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            Conn::Revoked { .. } | Conn::Refused { .. } | Conn::NoToken
        )
    }

    pub fn label(&self) -> &'static str {
        match self {
            Conn::NoToken => "Not signed in",
            Conn::Connecting => "Connecting",
            Conn::Open => "Connected",
            Conn::Reconnecting { .. } => "Reconnecting",
            Conn::Refused { .. } => "Refused",
            Conn::Closed { .. } => "Disconnected",
            Conn::Revoked { .. } => "Access revoked",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Store {
    pub conn: RwSignal<Conn>,
    pub welcome: RwSignal<Option<Arc<Welcome>>>,
    pub schema: RwSignal<Option<Arc<Schema>>>,
    pub state: RwSignal<Option<Arc<State>>>,
    pub status: RwSignal<Option<Arc<Status>>>,
    pub status_at: RwSignal<f64>,
    pub rates: RwSignal<Rates>,
    pub recorder: RwSignal<Recorder>,
    pub recorded_from: RwSignal<f64>,
    pub toasts: RwSignal<Vec<Toast>>,
    pub toast_hold: RwSignal<bool>,
    pub flow_open: RwSignal<bool>,
    pub search_open: RwSignal<bool>,
    pub creating: RwSignal<Option<String>>,
    pub editing: RwSignal<bool>,
    pub values_at: RwSignal<f64>,
    pub retry_at: RwSignal<f64>,
    pub now: RwSignal<f64>,
    pub history: RwSignal<Option<Vec<AuditEntry>>>,
    pub changed: RwSignal<Option<ConfigEvent>>,
    pub changed_from: RwSignal<(String, f64)>,
    pub added_at: RwSignal<std::collections::BTreeMap<String, f64>>,
    pub route: RwSignal<Route>,
    pub prev: RwSignal<Route>,
    pub server_addr: RwSignal<String>,
    pub error: RwSignal<Option<String>>,
    pub edit: Edit,
}

impl Store {
    pub fn new(route: Route) -> Self {
        Store {
            conn: RwSignal::new(Conn::Connecting),
            welcome: RwSignal::new(None),
            schema: RwSignal::new(None),
            state: RwSignal::new(None),
            status: RwSignal::new(None),
            status_at: RwSignal::new(0.0),
            rates: RwSignal::new(Rates::default()),
            recorder: RwSignal::new(Recorder::default()),
            recorded_from: RwSignal::new(0.0),
            toasts: RwSignal::new(Vec::new()),
            toast_hold: RwSignal::new(false),
            flow_open: RwSignal::new(false),
            search_open: RwSignal::new(false),
            creating: RwSignal::new(None),
            editing: RwSignal::new(false),
            values_at: RwSignal::new(0.0),
            retry_at: RwSignal::new(0.0),
            now: RwSignal::new(js_sys::Date::now()),
            history: RwSignal::new(None),
            changed: RwSignal::new(None),
            changed_from: RwSignal::new((String::new(), 0.0)),
            added_at: RwSignal::new(std::collections::BTreeMap::new()),
            prev: RwSignal::new(Route::Overview),
            server_addr: RwSignal::new(String::new()),
            route: RwSignal::new(route),
            error: RwSignal::new(None),
            edit: Edit::new(),
        }
    }

    pub fn take_status(&self, next: Status, now_ms: f64) {
        let prev = self.status.get_untracked();
        let prev_at = self.status_at.get_untracked();
        if let Some(prev) = prev {
            let secs = ((now_ms - prev_at) / 1000.0).max(0.001);
            let r = rates(&prev, &next, secs);
            let missed = ((now_ms - prev_at) / 1000.0).round() as i64 - 1;
            self.recorder.update(|h| {
                for _ in 0..missed.clamp(0, 900) {
                    h.gap();
                }
                h.record(&r);
            });
            if self.recorded_from.get_untracked() == 0.0 {
                self.recorded_from.set(now_ms);
            }
            self.rates.set(r);
        }
        self.values_at.set(now_ms);
        self.status.set(Some(Arc::new(next)));
        self.status_at.set(now_ms);
    }

    pub fn can_write_now(&self) -> bool {
        untrack(|| self.can_write())
    }

    pub fn can_write(&self) -> bool {
        self.welcome.with(|w| {
            w.as_ref()
                .is_some_and(|w| w.you.level == burrow_admin_proto::msg::AdminLevel::Write)
        }) && self
            .state
            .with(|s| s.as_ref().is_some_and(|s| s.file.writable))
    }
}

impl Store {
    pub fn toast(&self, tone: Tone, text: impl Into<String>, action: Option<&str>) {
        let id = js_sys::Date::now() as u64 ^ self.toasts.with_untracked(|t| t.len() as u64);
        self.toasts.update(|t| {
            t.push(Toast {
                id,
                tone,
                text: text.into(),
                action: action.map(str::to_string),
            });
            let excess = t.len().saturating_sub(3);
            t.drain(..excess);
        });
        let me = *self;
        set_timeout(move || me.drop_toast(id), std::time::Duration::from_secs(5));
    }

    fn drop_toast(&self, id: u64) {
        if self.toast_hold.try_get_untracked() == Some(true) {
            let me = *self;
            set_timeout(move || me.drop_toast(id), std::time::Duration::from_secs(1));
            return;
        }
        let _ = self.toasts.try_update(|t| t.retain(|x| x.id != id));
    }

    pub fn running(&self) -> burrow_admin_proto::serde_json::Value {
        self.state.with(|s| {
            s.as_ref()
                .map(|s| s.values.clone())
                .unwrap_or(burrow_admin_proto::serde_json::Value::Null)
        })
    }

    pub fn server_name(&self) -> String {
        let public = self.state.with(|s| {
            s.as_ref().and_then(|s| {
                ServerConfig::PUBLIC_ADDRESS
                    .str(&s.values)
                    .map(str::to_string)
            })
        });
        public
            .filter(|p| !p.is_empty())
            .or_else(|| Some(self.server_addr.get()).filter(|a| !a.is_empty()))
            .unwrap_or_else(|| "burrow server".into())
    }

    pub fn note_clients(&self, next: &State) {
        use burrow_config::{ClientEntry, Entry};
        let names = |v: &burrow_admin_proto::serde_json::Value| -> Vec<String> {
            ClientEntry::entries(v)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default()
        };
        let Some(before) = self
            .state
            .with_untracked(|s| s.as_ref().map(|s| names(&s.values)))
        else {
            return;
        };
        let fresh: Vec<String> = names(&next.values)
            .into_iter()
            .filter(|n| !before.contains(n))
            .collect();
        if !fresh.is_empty() {
            let now = js_sys::Date::now();
            self.added_at.update(|m| {
                for n in fresh {
                    m.insert(n, now);
                }
            });
        }
    }

    pub fn added_secs(&self, client: &str) -> Option<u64> {
        let at = self.added_at.with(|m| m.get(client).copied())?;
        Some(((self.now.get() - at) / 1000.0).max(0.0) as u64)
    }

    pub fn me(&self) -> Option<String> {
        self.welcome
            .with(|w| w.as_ref().map(|w| w.you.visitor.clone()))
    }

    pub fn in_use(&self, v: &burrow_admin_proto::status::VisitorStatus) -> bool {
        v.open > 0 || self.me().as_deref() == Some(v.name.as_str())
    }

    pub fn short_rev(&self) -> String {
        self.state
            .with(|s| s.as_ref().map(|s| short_rev(&s.rev)).unwrap_or_default())
    }
}

pub fn short_rev(rev: &str) -> String {
    rev.trim_start_matches("b2s:").chars().take(8).collect()
}

pub fn store() -> Store {
    expect_context::<Store>()
}

pub fn try_store() -> Option<Store> {
    use_context::<Store>()
}
