use std::sync::Arc;

use burrow_admin_logic::rates::{History, Rates, rates};
use burrow_admin_proto::msg::{AuditEntry, ConfigEvent, State, Welcome};
use burrow_admin_proto::schema::Schema;
use burrow_admin_proto::status::Status;
use leptos::prelude::*;

use crate::edit::Edit;
use crate::route::Route;

pub const SPARK_POINTS: usize = 600;

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
    pub traffic_in: RwSignal<History>,
    pub traffic_out: RwSignal<History>,
    pub history: RwSignal<Option<Vec<AuditEntry>>>,
    pub changed: RwSignal<Option<ConfigEvent>>,
    pub route: RwSignal<Route>,
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
            traffic_in: RwSignal::new(History::new(SPARK_POINTS)),
            traffic_out: RwSignal::new(History::new(SPARK_POINTS)),
            history: RwSignal::new(None),
            changed: RwSignal::new(None),
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
            self.traffic_in.update(|h| h.push(r.total.bytes_in));
            self.traffic_out.update(|h| h.push(r.total.bytes_out));
            self.rates.set(r);
        }
        self.status.set(Some(Arc::new(next)));
        self.status_at.set(now_ms);
    }

    pub fn can_write_now(&self) -> bool {
        untrack(|| self.can_write())
    }

    pub fn can_write(&self) -> bool {
        self.welcome.with(|w| {
            w.as_ref()
                .is_some_and(|w| w.you.level == burrow_admin_proto::msg::Level::Write)
        }) && self
            .state
            .with(|s| s.as_ref().is_some_and(|s| s.file.writable))
    }
}

pub fn store() -> Store {
    expect_context::<Store>()
}

pub fn try_store() -> Option<Store> {
    use_context::<Store>()
}
