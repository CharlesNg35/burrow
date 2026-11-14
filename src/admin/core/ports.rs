use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::Format;
use crate::config::docs::Docs;
use crate::config::settings::ServerSettings;
use crate::metrics::Status;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    Changed,
    Io(String),
}

pub use burrow_admin_proto::msg::WhyNot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub rev: String,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Noted {
    pub entries: Vec<(u128, burrow_admin_proto::msg::AuditEntry)>,
    pub kept: Option<u128>,
}

impl Noted {
    pub fn by_hand(v: &Version) -> Noted {
        Noted {
            entries: Vec::new(),
            kept: Some(u128::from(v.at) * 1_000_000_000),
        }
    }
}

pub trait Store: Send + Sync {
    fn path(&self) -> &Path;
    fn state_dir(&self) -> PathBuf;
    fn log_dir(&self) -> Option<PathBuf>;
    fn format(&self) -> Format;
    fn read(&self) -> std::io::Result<Docs>;
    fn write(&self, before: &Docs, after: &Docs) -> Result<(), WriteError>;
    fn writable(&self) -> Result<(), WhyNot>;
    fn keep(&self, rev: &str, docs: &Docs);
    fn versions(&self) -> Vec<Version>;
    fn version(&self, rev: &str) -> Option<Docs>;
    fn history_dir(&self) -> Option<PathBuf>;
    fn history_problem(&self) -> Option<String>;
    fn read_only(&self, _name: &str) -> Option<String> {
        None
    }
    fn clash(&self, _name: &str) -> Option<String> {
        None
    }
    fn note(&self, _entry: &burrow_admin_proto::msg::AuditEntry) {}
    fn noted(&self, v: &Version) -> Noted {
        Noted::by_hand(v)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub client: String,
    pub secret_fp: String,
    pub from: String,
    pub over_websocket: bool,
}

pub trait Live: Send + Sync {
    fn status(&self) -> Status;
    fn settings(&self) -> Arc<ServerSettings>;
    fn sessions(&self) -> Vec<LiveSession>;
    fn allows(&self) -> BTreeMap<String, Vec<String>>;
    fn free_port(&self, near: u16, count: u16, udp: bool) -> Option<u16>;
    fn bind_probe(
        &self,
        _proto: burrow_config::Protocol,
        _addr: std::net::SocketAddr,
    ) -> std::io::Result<()> {
        Ok(())
    }
    fn latest(&self) -> burrow_admin_proto::updates::LatestRelease {
        burrow_admin_proto::updates::LatestRelease::default()
    }
    fn latest_now(&self) -> burrow_admin_proto::updates::LatestRelease {
        self.latest()
    }
    fn ask_update(&self, _client: &str, _to: &str, _by: &str) -> Result<(), String> {
        Err("this server cannot ask clients to update".into())
    }
    fn update_all(&self, _clients: Vec<String>, _to: &str, _by: &str) -> Result<(), String> {
        Err("this server cannot ask clients to update".into())
    }
    fn own_update(&self) -> Option<burrow_admin_proto::updates::ServerUpdate> {
        None
    }
    fn update_self(&self, _by: &str) -> Result<(), String> {
        Err("this server cannot update itself from burrow admin".into())
    }
}

pub trait Env: crate::config::env::EnvSource + Send + Sync {}

impl<T: crate::config::env::EnvSource + Send + Sync> Env for T {}
