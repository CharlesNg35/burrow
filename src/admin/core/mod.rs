pub mod diags;
pub mod diff;
mod dispatch;
pub mod guard;
mod history;
mod older;
pub mod ops;
pub mod passwords;
pub mod plan;
pub mod ports;
pub mod snippets;
mod status;
pub mod summary;
pub mod text;
pub mod uploads;
pub mod values;

pub use dispatch::{dispatch, ok, parse};

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use burrow_admin_proto::msg::{
    AdminLevel, AuditEntry, ConfigFile, ErrorBody, ErrorCode, FileInfo, GeneratedSecret, Limits,
    Running, Runtime, SchemaReply, ServerInfo, SessionInfo, State, Suggest, Suggestions, Welcome,
    WhyNot, You,
};
use burrow_admin_proto::schema::{Entity, Schema};
use burrow_config::{ClientEntry, Entry, Protocol, ServerConfig, ServiceEntry, VisitorEntry};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::config::docs::Docs;
use crate::wire::secret::Secret;
use ports::{Env, Live, Store};

pub const MAX_PENDING_SECRETS: usize = 16;
pub const SECRET_PREFIX: &str = "s";
pub const PENDING_SECRET_TTL: Duration = Duration::from_secs(30 * 60);
const AUDIT_RING: usize = 200;

#[derive(Debug)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub body: Value,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ApiError {
            code,
            message: message.into(),
            body: Value::Null,
        }
    }

    pub fn with(mut self, body: Value) -> Self {
        self.body = body;
        self
    }

    pub fn wire(&self) -> ErrorBody {
        ErrorBody {
            code: self.code,
            message: self.message.clone(),
            body: self.body.clone(),
        }
    }
}

pub type Reply = Result<Value, ApiError>;

pub struct Pending {
    pub handle: String,
    pub value: Zeroizing<String>,
    pub fp: String,
    pub at: Instant,
}

pub struct Session {
    pub visitor: String,
    pub level: AdminLevel,
    pub peer: SocketAddr,
    pub secret: Secret,
    pub via_websocket: bool,
    pub client: String,
    ui_version: Mutex<Option<String>>,
    understands: Mutex<Vec<String>>,
    pending: Mutex<Vec<Pending>>,
    next_handle: AtomicU64,
}

impl Session {
    pub fn new(
        visitor: String,
        level: AdminLevel,
        peer: SocketAddr,
        secret: Secret,
        via_websocket: bool,
        client: String,
    ) -> Self {
        Session {
            visitor,
            level,
            peer,
            secret,
            via_websocket,
            client,
            ui_version: Mutex::new(None),
            understands: Mutex::new(Vec::new()),
            pending: Mutex::new(Vec::new()),
            next_handle: AtomicU64::new(0),
        }
    }

    fn prune(&self) {
        let mut p = self.pending.lock().expect("pending lock");
        p.retain(|s| s.at.elapsed() < PENDING_SECRET_TTL);
    }

    fn full(pending: &[Pending]) -> Result<(), ApiError> {
        if pending.len() < MAX_PENDING_SECRETS {
            return Ok(());
        }
        Err(ApiError::new(
            ErrorCode::Busy,
            format!(
                "at most {MAX_PENDING_SECRETS} unused secrets and passwords per session; apply or reload the page"
            ),
        ))
    }

    fn has_room(&self) -> Result<(), ApiError> {
        self.prune();
        Session::full(&self.pending.lock().expect("pending lock"))
    }

    fn hold(&self, prefix: &str, value: Zeroizing<String>, fp: String) -> Result<String, ApiError> {
        self.prune();
        let mut pending = self.pending.lock().expect("pending lock");
        Session::full(&pending)?;
        let n = self.next_handle.fetch_add(1, Ordering::Relaxed) + 1;
        let handle = format!("{prefix}{n}");
        pending.push(Pending {
            handle: handle.clone(),
            value,
            fp,
            at: Instant::now(),
        });
        Ok(handle)
    }

    pub fn pending_secret(&self, handle: &str) -> Option<(String, String)> {
        self.prune();
        self.pending
            .lock()
            .expect("pending lock")
            .iter()
            .find(|p| p.handle == handle && !p.handle.starts_with(passwords::PREFIX))
            .map(|p| (p.value.to_string(), p.fp.clone()))
    }

    pub fn pending_password(&self, handle: &str) -> Option<String> {
        self.prune();
        self.pending
            .lock()
            .expect("pending lock")
            .iter()
            .find(|p| p.handle == handle && p.handle.starts_with(passwords::PREFIX))
            .map(|p| p.value.to_string())
    }

    pub fn consume(&self, handles: &[String]) {
        self.pending
            .lock()
            .expect("pending lock")
            .retain(|p| !handles.contains(&p.handle));
    }

    pub fn writes(&self) -> bool {
        self.level == AdminLevel::Write
    }

    pub fn hello(&self, burrow: Option<String>, understands: Vec<String>) {
        *self.ui_version.lock().expect("ui version lock") = burrow;
        *self.understands.lock().expect("understands lock") = understands;
    }

    pub fn understands(&self, feature: &str) -> bool {
        self.understands
            .lock()
            .expect("understands lock")
            .iter()
            .any(|f| f == feature)
    }

    pub fn older_ui(&self) -> bool {
        let ui = burrow_admin_proto::local::UI_CLIENT;
        let is_ui = self.client == ui || self.client.starts_with(&format!("{ui}/"));
        is_ui && self.ui_version.lock().expect("ui version lock").is_none()
    }
}

pub struct Core {
    pub store: Box<dyn Store>,
    pub live: Box<dyn Live>,
    pub env: Box<dyn Env>,
    schema: Schema,
    audit: Mutex<VecDeque<AuditEntry>>,
    last_good: Mutex<Option<String>>,
    seen: Mutex<VecDeque<(String, Docs)>>,
    started: Instant,
}

const SEEN: usize = 8;

pub fn features() -> Vec<String> {
    burrow_admin_proto::features::ALL
        .iter()
        .map(|s| s.to_string())
        .collect()
}

impl Core {
    pub fn new(store: Box<dyn Store>, live: Box<dyn Live>, env: Box<dyn Env>) -> Self {
        let mut audit = VecDeque::new();
        for v in store.versions().into_iter().take(AUDIT_RING).rev() {
            let id = next_id(audit.front(), v.at);
            audit.push_front(AuditEntry {
                id,
                at: v.at,
                visitor: String::new(),
                from: String::new(),
                base: String::new(),
                rev: v.rev,
                changes: Vec::new(),
                result: "kept".into(),
                revertible: true,
                details_kept: false,
                reverts: None,
            });
        }
        Core {
            store,
            live,
            env,
            schema: crate::admin::hooks::schema(crate::config::admin_schema::server_schema()),
            audit: Mutex::new(audit),
            last_good: Mutex::new(None),
            seen: Mutex::new(VecDeque::new()),
            started: Instant::now(),
        }
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    pub fn welcome(&self, s: &Session) -> Welcome {
        let settings = self.live.settings();
        let name_hint = settings.public_address.clone().unwrap_or_default();
        Welcome {
            server: ServerInfo {
                version: crate::admin::hooks::version(env!("CARGO_PKG_VERSION")),
                target: env!("BURROW_TARGET").into(),
                uptime_secs: self.started.elapsed().as_secs(),
                name_hint,
            },
            protocol: burrow_admin_proto::PROTOCOL_MAX,
            you: You {
                visitor: s.visitor.clone(),
                level: s.level,
                from: s.peer.ip().to_string(),
                secret_fp: s.secret.fingerprint(),
            },
            features: features(),
            limits: Limits {
                max_message: burrow_admin_proto::frame::MAX_MESSAGE as u64,
                max_inflight: super::MAX_INFLIGHT as u32,
            },
            schema_hash: self.schema.hash.clone(),
        }
    }

    fn schema_reply(&self, have: &str) -> SchemaReply {
        if have == self.schema.hash {
            SchemaReply {
                hash: have.to_string(),
                schema: None,
                unchanged: true,
            }
        } else {
            SchemaReply {
                hash: self.schema.hash.clone(),
                schema: Some(self.schema.clone()),
                unchanged: false,
            }
        }
    }

    #[cfg(test)]
    pub fn read(&self) -> Result<String, ApiError> {
        self.docs().map(|d| d.main().text.clone())
    }

    pub fn docs(&self) -> Result<Docs, ApiError> {
        self.store.read().map_err(|e| {
            ApiError::new(
                ErrorCode::Internal,
                format!(
                    "cannot read {}: {}",
                    self.store.path().display(),
                    crate::base::failure::file_reason(&e)
                ),
            )
        })
    }

    fn file_info(&self, s: &Session, docs: &Docs) -> FileInfo {
        let writable = self.store.writable();
        FileInfo {
            path: self.store.path().display().to_string(),
            format: match self.store.format() {
                crate::config::Format::Yaml => "yaml".into(),
                crate::config::Format::Json => "json".into(),
            },
            writable: writable.is_ok() && s.writes(),
            why_not: match (&writable, s.writes()) {
                (Err(w), _) => Some(w.clone()),
                (Ok(()), false) => Some(WhyNot {
                    code: burrow_admin_proto::codes::READ_LEVEL.into(),
                    message: format!("visitor \"{}\" has read access only", s.visitor),
                    hint: format!(
                        "to change the config from here, set \"admin: write\" for visitor \"{}\" in the server config",
                        s.visitor
                    ),
                }),
                (Ok(()), true) => None,
            },
            source_differs: None,
            why_not_history: self.store.history_problem(),
            history_dir: self.store.history_dir().map(|d| d.display().to_string()),
            files: self.config_files(docs),
        }
    }

    fn config_files(&self, docs: &Docs) -> Vec<ConfigFile> {
        if docs.list.len() < 2 {
            return Vec::new();
        }
        let owners = plan::files::Owners::of(docs);
        docs.list
            .iter()
            .enumerate()
            .map(|(i, d)| ConfigFile {
                name: d.name.clone(),
                read_only: self.store.read_only(&d.name),
                items: owners
                    .items
                    .iter()
                    .filter(|(_, at)| **at == i)
                    .map(|((section, name), _)| vec![section.clone(), name.clone()])
                    .collect(),
            })
            .collect()
    }

    pub fn seen_docs(&self, rev: &str) -> Option<Docs> {
        self.seen
            .lock()
            .expect("seen lock")
            .iter()
            .find(|(r, _)| r == rev)
            .map(|(_, t)| t.clone())
    }

    pub fn state(&self, s: &Session) -> Result<State, ApiError> {
        let docs = self.docs()?;
        let rev = text::docs_rev(&docs);
        {
            let mut seen = self.seen.lock().expect("seen lock");
            if !seen.iter().any(|(r, _)| *r == rev) {
                seen.push_front((rev.clone(), docs.clone()));
                seen.truncate(SEEN);
            }
        }
        let checked = plan::check_docs(self, &docs);
        if checked.valid {
            *self.last_good.lock().expect("last good lock") = Some(rev.clone());
        }
        let values = checked
            .raw
            .as_ref()
            .map(|raw| values::present(raw, Some(&self.schema.root), "", &*self.env))
            .unwrap_or(Value::Null);
        let running_settings = self.live.settings();
        let mut restart_pending = Vec::new();
        if let Some(file) = &checked.settings
            && file.listen != running_settings.listen
        {
            restart_pending.push(ServerConfig::LISTEN.key().to_string());
        }
        let status = self.live.status();
        let mut bound = BTreeMap::new();
        for svc in &status.services {
            if let Some(b) = &svc.bind {
                bound.insert(svc.name.clone(), vec![b.clone()]);
            }
        }
        let sessions = self
            .live
            .sessions()
            .into_iter()
            .map(|l| {
                (
                    l.client,
                    SessionInfo {
                        secret_fp: l.secret_fp,
                        from: l.from,
                    },
                )
            })
            .collect();
        let notice = older::notice(self, s, checked.raw.as_ref());
        let (errors, mut warnings): (Vec<_>, Vec<_>) =
            checked.diags.into_iter().partition(|d| d.is_error());
        warnings.extend(notice);
        Ok(State {
            rev: rev.clone(),
            file: self.file_info(s, &docs),
            running: Running {
                rev: self
                    .last_good
                    .lock()
                    .expect("last good lock")
                    .clone()
                    .unwrap_or_default(),
                ok: errors.is_empty(),
                file_diags: errors,
            },
            values,
            runtime: Runtime {
                sessions,
                addresses: addresses(&running_settings),
                bound,
                restart_pending,
                allows: self.live.allows(),
            },
            diags: warnings,
        })
    }

    pub fn generate_secret(&self, s: &Session) -> Result<GeneratedSecret, ApiError> {
        if !s.writes() {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "read access cannot generate secrets",
            ));
        }
        let secret = Secret::generate();
        let fp = secret.fingerprint();
        let handle = s.hold(SECRET_PREFIX, Zeroizing::new(secret.encode()), fp.clone())?;
        Ok(GeneratedSecret { handle, fp })
    }

    pub fn suggest(&self, req: &Suggest) -> Result<Suggestions, ApiError> {
        match req.what.as_str() {
            burrow_admin_proto::msg::suggest::FREE_PORT => {
                let near = req
                    .near
                    .unwrap_or(burrow_syntax::ports::FIRST_SERVICE_PORT.into())
                    .clamp(1024, 65535) as u16;
                let count = req.count.unwrap_or(1).clamp(1, 1000) as u16;
                let udp = req.proto.as_deref().and_then(Protocol::from_wire) == Some(Protocol::Udp);
                let docs = self.docs()?;
                let used = plan::used_ports(self, &docs);
                let mut at = near;
                for _ in 0..200 {
                    match self.live.free_port(at, count, udp) {
                        Some(p) if !(p..p.saturating_add(count)).any(|x| used.contains(&x)) => {
                            return Ok(Suggestions {
                                values: vec![Value::from(p)],
                            });
                        }
                        Some(p) => at = p.saturating_add(count),
                        None => break,
                    }
                }
                Ok(Suggestions { values: Vec::new() })
            }
            burrow_admin_proto::msg::suggest::NAME => {
                let docs = self.docs()?;
                let checked = plan::check_docs(self, &docs);
                let section = match req.entity.as_deref().map(Entity::parse) {
                    Some(Entity::Client) => ClientEntry::section(),
                    Some(Entity::Visitor) => VisitorEntry::section(),
                    _ => ServiceEntry::section(),
                };
                let taken: Vec<String> = checked
                    .raw
                    .as_ref()
                    .and_then(|r| r.get(section))
                    .and_then(Value::as_object)
                    .map(|m| m.keys().cloned().collect())
                    .unwrap_or_default();
                let refs: Vec<&str> = taken.iter().map(String::as_str).collect();
                let base = req.base.clone().unwrap_or_else(|| "new".into());
                Ok(Suggestions {
                    values: vec![Value::from(burrow_syntax::name::suggest(&base, &refs))],
                })
            }
            other => Err(ApiError::new(
                ErrorCode::BadRequest,
                format!("cannot suggest \"{other}\""),
            )),
        }
    }

    pub fn record(&self, mut entry: AuditEntry) {
        let mut a = self.audit.lock().expect("audit lock");
        entry.id = next_id(a.front(), entry.at);
        a.push_front(entry);
        a.truncate(AUDIT_RING);
    }
}

fn next_id(newest: Option<&AuditEntry>, at: u64) -> u64 {
    let after = newest.map_or(0, |e| e.id).saturating_add(1);
    after.max(at.saturating_mul(1000))
}

fn addresses(s: &crate::config::settings::ServerSettings) -> Vec<String> {
    let mut out = vec![
        "0.0.0.0".to_string(),
        "::".to_string(),
        "127.0.0.1".to_string(),
    ];
    let mut add = |ip: std::net::IpAddr| {
        let t = ip.to_string();
        if !out.contains(&t) {
            out.push(t);
        }
    };
    if let Some(l) = s.listen {
        add(l.ip());
    }
    if let Some(w) = &s.websocket {
        add(w.listen.ip());
    }
    for svc in s.services.values() {
        if let Some(b) = svc.bind {
            add(b.ip());
        }
    }
    if let Some(ip) = s
        .public_address
        .as_deref()
        .and_then(|a| a.parse::<std::net::IpAddr>().ok())
    {
        add(ip);
    }
    out
}

#[cfg(test)]
#[cfg(feature = "web")]
mod agree;
#[cfg(test)]
pub(crate) mod tests;
