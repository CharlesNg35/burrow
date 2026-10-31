mod files;
mod locks;
mod locks_web;
mod older;
mod passwords;
mod plans;
mod reach;
mod refs;
mod requests;
mod reverts;
mod snippets;
mod status;
mod uploads;
mod web_ports;
mod websocket;
mod wording;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use burrow_admin_proto::msg::{AdminLevel, Apply};
use burrow_admin_proto::ops::Op;

use crate::config::settings::ServerSettings;
use crate::config::{Format, Loaded, Role, load_str};
use crate::metrics::Status;

use super::plan::{self, ReloadResult};
use super::ports::{Live, LiveSession, Store, Version, WhyNot, WriteError};
use super::*;

pub struct MemStore {
    pub text: Arc<Mutex<String>>,
    pub kept: Mutex<Vec<(Version, String)>>,
    pub read_only: bool,
    pub state: tempfile::TempDir,
    pub asked: Arc<AtomicUsize>,
}

impl Store for MemStore {
    fn path(&self) -> &Path {
        Path::new("server.yaml")
    }
    fn state_dir(&self) -> PathBuf {
        self.state.path().to_path_buf()
    }
    fn format(&self) -> Format {
        Format::Yaml
    }
    fn read(&self) -> std::io::Result<String> {
        Ok(self.text.lock().unwrap().clone())
    }
    fn write(&self, before: &str, after: &str) -> Result<(), WriteError> {
        let mut t = self.text.lock().unwrap();
        if *t != before {
            return Err(WriteError::Changed);
        }
        *t = after.to_string();
        Ok(())
    }
    fn writable(&self) -> Result<(), WhyNot> {
        if self.read_only {
            return Err(WhyNot {
                code: burrow_admin_proto::codes::READ_ONLY_FS.into(),
                message: "read only".into(),
                hint: String::new(),
            });
        }
        Ok(())
    }
    fn keep(&self, rev: &str, text: &str) {
        let mut k = self.kept.lock().unwrap();
        if !k.iter().any(|(v, _)| v.rev == rev) {
            let at = k.len() as u64;
            k.insert(
                0,
                (
                    Version {
                        rev: rev.into(),
                        at,
                    },
                    text.into(),
                ),
            );
            k.truncate(20);
        }
    }
    fn versions(&self) -> Vec<Version> {
        self.kept
            .lock()
            .unwrap()
            .iter()
            .map(|(v, _)| v.clone())
            .collect()
    }
    fn version(&self, rev: &str) -> Option<String> {
        self.asked.fetch_add(1, Ordering::Relaxed);
        self.kept
            .lock()
            .unwrap()
            .iter()
            .find(|(v, _)| v.rev == rev)
            .map(|(_, t)| t.clone())
    }
    fn history_dir(&self) -> Option<PathBuf> {
        Some(self.state.path().join(".server.yaml.history")).filter(|d| d.is_dir())
    }
    fn history_problem(&self) -> Option<String> {
        None
    }
}

pub struct FakeLive {
    pub text: Arc<Mutex<String>>,
    pub status: Mutex<Status>,
    pub sessions: Mutex<Vec<LiveSession>>,
    pub busy: Vec<u16>,
    pub allows: BTreeMap<String, Vec<String>>,
}

impl Live for FakeLive {
    fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
    fn settings(&self) -> Arc<ServerSettings> {
        Arc::new(settings_of(&self.text.lock().unwrap()))
    }
    fn sessions(&self) -> Vec<LiveSession> {
        self.sessions.lock().unwrap().clone()
    }
    fn allows(&self) -> BTreeMap<String, Vec<String>> {
        self.allows.clone()
    }
    fn free_port(&self, near: u16, _: u16, _: bool) -> Option<u16> {
        (!self.busy.contains(&near)).then_some(near)
    }
    fn bind_probe(&self, addr: std::net::SocketAddr) -> std::io::Result<()> {
        match self.busy.contains(&addr.port()) {
            true => Err(std::io::ErrorKind::AddrInUse.into()),
            false => Ok(()),
        }
    }
}

pub struct World {
    pub ops: Secret,
    pub home: Secret,
    pub text: String,
}

pub fn world() -> World {
    let (ops, home) = (Secret::generate(), Secret::generate());
    let text = format!(
        "# my server\nlisten: 7835 # tcp\n\nclients:\n  # the home box\n  home:\n    secret: {}\n\nvisitors:\n  ops:\n    secret: {}\n    admin: write\n\nservices:\n  web: {{ client: home, bind: 7840, target: 3000 }}\n",
        home.encode(),
        ops.encode()
    );
    World { ops, home, text }
}

fn settings_of(text: &str) -> ServerSettings {
    let nothing = |_: &str| None;
    let o = load_str(
        "server.yaml",
        text,
        Format::Yaml,
        Some(Role::Server),
        &nothing,
    );
    match o.loaded {
        Some(Loaded::Server(s, _)) => *s,
        _ => panic!("loads: {:#?}", o.report.diagnostics),
    }
}

pub fn core_for(text: &str) -> Core {
    core_running(text, text)
}

pub fn core_with_env(
    text: &str,
    running: &str,
    vars: &'static [(&'static str, &'static str)],
) -> Core {
    let mut core = core_running(text, running);
    core.env = Box::new(move |name: &str| {
        vars.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    });
    core
}

pub fn core_running(text: &str, running: &str) -> Core {
    core_busy(text, running, Vec::new())
}

pub fn core_busy(text: &str, running: &str, busy: Vec<u16>) -> Core {
    core_of(text, running, busy, Vec::new())
}

pub fn core_of(text: &str, running: &str, busy: Vec<u16>, sessions: Vec<LiveSession>) -> Core {
    core_counted(text, running, busy, sessions).0
}

pub fn core_counted(
    text: &str,
    running: &str,
    busy: Vec<u16>,
    sessions: Vec<LiveSession>,
) -> (Core, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    let shared = Arc::new(Mutex::new(text.to_string()));
    let live_text = if text == running {
        shared.clone()
    } else {
        Arc::new(Mutex::new(running.to_string()))
    };
    let store = MemStore {
        text: shared.clone(),
        kept: Mutex::new(Vec::new()),
        read_only: false,
        state: tempfile::tempdir().unwrap(),
        asked: asked.clone(),
    };
    let live = FakeLive {
        text: live_text,
        status: Mutex::new(Status::default()),
        sessions: Mutex::new(sessions),
        busy,
        allows: BTreeMap::new(),
    };
    let core = Core::new(Box::new(store), Box::new(live), Box::new(|_: &str| None));
    (core, asked)
}

pub fn session(w: &World, level: AdminLevel) -> Session {
    Session::new(
        "ops".into(),
        level,
        "203.0.113.7:5000".parse().unwrap(),
        w.ops.clone(),
        false,
        "test".into(),
    )
}

fn p(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

fn apply_now(
    core: &Core,
    s: &Session,
    ops: &[Op],
    confirm: &[&str],
) -> Result<burrow_admin_proto::plan::Applied, ApiError> {
    apply_at(core, s, ops, confirm, None)
}

fn apply_at(
    core: &Core,
    s: &Session,
    ops: &[Op],
    confirm: &[&str],
    address: Option<&str>,
) -> Result<burrow_admin_proto::plan::Applied, ApiError> {
    let base = text::rev(&core.read()?);
    let plan = plan::compute(core, s, &base, ops)?.plan;
    let begun = plan::apply_begin(
        core,
        s,
        &Apply {
            base,
            ops: ops.to_vec(),
            plan: plan.plan,
            confirm: confirm.iter().map(|c| c.to_string()).collect(),
            address: address.map(str::to_string),
        },
    )?;
    plan::finish(core, s, begun, ReloadResult::unchanged(), None)
}
