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
    Ended,
}

impl Conn {
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            Conn::Revoked { .. } | Conn::Refused { .. } | Conn::NoToken | Conn::Ended
        )
    }

    pub fn shape(&self) -> Conn {
        match self {
            Conn::Reconnecting { .. } => Conn::Reconnecting { attempt: 0 },
            other => other.clone(),
        }
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
            Conn::Ended => "Session ended",
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
    pub next_toast: StoredValue<u64>,
    pub flow_open: RwSignal<bool>,
    pub search_open: RwSignal<bool>,
    pub creating: RwSignal<Option<String>>,
    pub editing: RwSignal<bool>,
    pub values_at: RwSignal<f64>,
    pub retry_at: RwSignal<f64>,
    pub admin_down: RwSignal<bool>,
    pub now: RwSignal<f64>,
    pub history: RwSignal<Option<Vec<AuditEntry>>>,
    pub changed: RwSignal<Option<ConfigEvent>>,
    pub changed_from: RwSignal<(String, f64)>,
    pub added_at: RwSignal<std::collections::BTreeMap<String, f64>>,
    pub rev_at: RwSignal<std::collections::BTreeMap<String, f64>>,
    pub raw_error: RwSignal<Option<String>>,
    pub route: RwSignal<Route>,
    pub query_seen: RwSignal<u64>,
    pub prev: RwSignal<Route>,
    pub server_addr: RwSignal<String>,
    pub error: RwSignal<Option<String>>,
    pub edit: Edit,
}

pub const REVIEW: &str = "Review";

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
            next_toast: StoredValue::new(0),
            flow_open: RwSignal::new(false),
            search_open: RwSignal::new(false),
            creating: RwSignal::new(None),
            editing: RwSignal::new(false),
            values_at: RwSignal::new(0.0),
            retry_at: RwSignal::new(0.0),
            admin_down: RwSignal::new(false),
            now: RwSignal::new(js_sys::Date::now()),
            history: RwSignal::new(None),
            changed: RwSignal::new(None),
            changed_from: RwSignal::new((String::new(), 0.0)),
            added_at: RwSignal::new(std::collections::BTreeMap::new()),
            rev_at: RwSignal::new(std::collections::BTreeMap::new()),
            raw_error: RwSignal::new(None),
            prev: RwSignal::new(Route::Overview),
            server_addr: RwSignal::new(String::new()),
            route: RwSignal::new(route),
            query_seen: RwSignal::new(0),
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
                for _ in 0..missed.clamp(0, burrow_admin_logic::rates::RECORD_SECONDS as i64) {
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

    pub fn files_on(&self) -> bool {
        self.welcome.with(|w| {
            w.as_ref()
                .is_some_and(|w| w.has(burrow_admin_proto::features::CONFIG_FILES))
        })
    }

    pub fn with_files<T>(
        &self,
        f: impl FnOnce(&burrow_admin_proto::msg::FileInfo) -> T,
    ) -> Option<T> {
        self.state.with(|s| s.as_ref().map(|s| f(&s.file)))
    }

    pub fn item_file(&self, item: &[String]) -> Option<String> {
        if !self.files_on() {
            return None;
        }
        let item = item.get(..2)?;
        let placed = self.edit.files.with(|f| f.placed(item).map(str::to_string));
        placed.or_else(|| self.with_files(|i| burrow_admin_logic::files::file_of(i, item)))
    }

    pub fn item_locked(&self, item: &[String]) -> Option<String> {
        let item = item.get(..2)?;
        self.with_files(|i| {
            burrow_admin_logic::files::locked_file(i, item)
                .map(burrow_admin_logic::files::read_only_line)
        })
        .flatten()
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
        let id = self.next_toast.get_value() + 1;
        self.next_toast.set_value(id);
        self.toasts.update(|t| {
            burrow_ui_kit::push_toast(
                t,
                Toast {
                    id,
                    tone,
                    text: text.into(),
                    action: action.map(str::to_string),
                },
            );
        });
        let me = *self;
        set_timeout(move || me.drop_toast(id), std::time::Duration::from_secs(5));
    }

    pub fn added_to_changes(&self) {
        self.toast(Tone::Success, "Added to changes", Some(REVIEW));
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
        use burrow_config::{ClientEntry, Entry, VisitorEntry};
        let names = |v: &burrow_admin_proto::serde_json::Value| -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            for (sec, m) in [
                (ClientEntry::section(), ClientEntry::entries(v)),
                (VisitorEntry::section(), VisitorEntry::entries(v)),
            ] {
                if let Some(m) = m {
                    out.extend(m.keys().map(|k| format!("{sec}/{k}")));
                }
            }
            out
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

    pub fn added_secs(&self, section: &str, name: &str) -> Option<u64> {
        let at = self
            .added_at
            .with(|m| m.get(&format!("{section}/{name}")).copied())?;
        Some(((self.now.get() - at) / 1000.0).max(0.0) as u64)
    }

    pub fn me(&self) -> Option<String> {
        self.welcome
            .with(|w| w.as_ref().map(|w| w.you.visitor.clone()))
    }

    pub fn in_use(&self, v: &burrow_admin_proto::status::VisitorStatus) -> bool {
        v.open > 0 || self.me().as_deref() == Some(v.name.as_str())
    }

    pub fn version(&self, rev: &str) -> String {
        let at = self
            .history
            .with(|h| {
                h.as_ref()
                    .and_then(|h| h.iter().find(|e| e.rev == rev))
                    .map(|e| e.at as f64 * 1000.0)
            })
            .or_else(|| self.rev_at.with(|m| m.get(rev).copied()))
            .or_else(|| self.started_with(rev));
        burrow_admin_logic::wording::version_name(at.map(crate::format::stamp).as_deref())
    }

    pub fn protocol_problem(&self, what: &str, e: impl std::fmt::Display) -> String {
        self.raw_error.set(Some(format!("{what}: {e}")));
        burrow_admin_logic::wording::PROTOCOL_ERROR.to_string()
    }

    fn started_with(&self, rev: &str) -> Option<f64> {
        let running = self
            .state
            .with(|s| s.as_ref().is_some_and(|s| s.running.rev == rev));
        let up = self.status.with(|s| s.as_ref().map(|s| s.uptime_secs))?;
        running.then(|| self.status_at.get() - up as f64 * 1000.0)
    }

    pub fn note_rev(&self, rev: &str) {
        let now = js_sys::Date::now();
        self.rev_at.update(|m| {
            m.entry(rev.to_string()).or_insert(now);
        });
    }

    pub fn current_version(&self) -> String {
        let rev = self.state.with(|s| s.as_ref().map(|s| s.rev.clone()));
        rev.map(|r| self.version(&r)).unwrap_or_default()
    }
}

pub fn short_rev(rev: &str) -> String {
    rev.trim_start_matches(burrow_admin_proto::REV_PREFIX)
        .chars()
        .take(8)
        .collect()
}

pub fn store() -> Store {
    expect_context::<Store>()
}

pub fn try_store() -> Option<Store> {
    use_context::<Store>()
}
