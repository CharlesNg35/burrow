use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::Format;
use crate::config::settings::ServerSettings;
use crate::metrics::Status;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    Changed,
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhyNot {
    pub code: String,
    pub message: String,
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub rev: String,
    pub at: u64,
}

pub trait Store: Send + Sync {
    fn path(&self) -> &Path;
    fn state_dir(&self) -> PathBuf;
    fn format(&self) -> Format;
    fn read(&self) -> std::io::Result<String>;
    fn write(&self, before: &str, after: &str) -> Result<(), WriteError>;
    fn writable(&self) -> Result<(), WhyNot>;
    fn keep(&self, rev: &str, text: &str);
    fn versions(&self) -> Vec<Version>;
    fn version(&self, rev: &str) -> Option<String>;
    fn history_dir(&self) -> Option<PathBuf>;
    fn history_problem(&self) -> Option<String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub client: String,
    pub secret_fp: String,
    pub from: String,
}

pub trait Live: Send + Sync {
    fn status(&self) -> Status;
    fn settings(&self) -> Arc<ServerSettings>;
    fn sessions(&self) -> Vec<LiveSession>;
    fn allows(&self) -> BTreeMap<String, Vec<String>>;
    fn free_port(&self, near: u16, count: u16, udp: bool) -> Option<u16>;
}

pub trait Env: crate::config::env::EnvSource + Send + Sync {}

impl<T: crate::config::env::EnvSource + Send + Sync> Env for T {}
