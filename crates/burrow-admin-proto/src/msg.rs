use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Path;
use crate::diag::Diag;
use crate::ops::Op;
use crate::plan::Change;

pub mod req {
    pub const WELCOME: &str = "welcome";
    pub const SCHEMA: &str = "schema";
    pub const STATE: &str = "state";
    pub const SUBSCRIBE: &str = "subscribe";
    pub const VALIDATE: &str = "validate";
    pub const APPLY: &str = "apply";
    pub const GENERATE_SECRET: &str = "generate_secret";
    pub const CONNECT_INFO: &str = "connect_info";
    pub const SUGGEST: &str = "suggest";
    pub const HISTORY: &str = "history";
    pub const REVERT: &str = "revert";
    pub const PING: &str = "ping";
}

pub mod ev {
    pub const STATUS: &str = "status";
    pub const CONFIG: &str = "config";
    pub const REVOKED: &str = "revoked";
    pub const BYE: &str = "bye";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u32,
    pub t: String,
    #[serde(default)]
    pub body: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub body: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Reply {
        re: u32,
        result: Result<Value, ErrorBody>,
    },
    Event {
        ev: String,
        body: Value,
    },
}

impl Incoming {
    pub fn parse(text: &str) -> Option<Incoming> {
        let v: Value = serde_json::from_str(text).ok()?;
        if let Some(ev) = v.get("ev").and_then(Value::as_str) {
            return Some(Incoming::Event {
                ev: ev.to_string(),
                body: v.get("body").cloned().unwrap_or(Value::Null),
            });
        }
        let re = u32::try_from(v.get("re")?.as_u64()?).ok()?;
        let ok = v.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let result = if ok {
            Ok(v.get("body").cloned().unwrap_or(Value::Null))
        } else {
            Err(v
                .get("err")
                .and_then(|e| serde_json::from_value(e.clone()).ok())
                .unwrap_or(ErrorBody {
                    code: ErrorCode::Unknown,
                    message: "the server sent an unreadable error".into(),
                    body: Value::Null,
                }))
        };
        Some(Incoming::Reply { re, result })
    }
}

pub fn reply_ok(re: u32, body: Value) -> Value {
    serde_json::json!({ "re": re, "ok": true, "body": body })
}

pub fn reply_err(re: u32, err: &ErrorBody) -> Value {
    serde_json::json!({ "re": re, "ok": false, "err": err })
}

pub fn event(ev: &str, body: Value) -> Value {
    serde_json::json!({ "ev": ev, "body": body })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    UnknownRequest,
    BadRequest,
    Forbidden,
    Conflict,
    Invalid,
    LockedOut,
    LockedField,
    NotWritable,
    Busy,
    TooLarge,
    ReloadFailed,
    Internal,
    #[serde(other)]
    Unknown,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::UnknownRequest => "unknown-request",
            ErrorCode::BadRequest => "bad-request",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Invalid => "invalid",
            ErrorCode::LockedOut => "locked-out",
            ErrorCode::LockedField => "locked-field",
            ErrorCode::NotWritable => "not-writable",
            ErrorCode::Busy => "busy",
            ErrorCode::TooLarge => "too-large",
            ErrorCode::ReloadFailed => "reload-failed",
            ErrorCode::Internal => "internal",
            ErrorCode::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockedField {
    pub path: crate::Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum AdminLevel {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Welcome {
    pub server: ServerInfo,
    pub protocol: u8,
    pub you: You,
    #[serde(default)]
    pub features: Vec<String>,
    pub limits: Limits,
    pub schema_hash: String,
}

impl Welcome {
    pub fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub version: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub uptime_secs: u64,
    #[serde(default)]
    pub name_hint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct You {
    pub visitor: String,
    pub level: AdminLevel,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub secret_fp: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub max_message: u64,
    pub max_inflight: u32,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SchemaRequest {
    #[serde(default)]
    pub have: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaReply {
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<crate::schema::Schema>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unchanged: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct State {
    pub rev: String,
    pub file: FileInfo,
    pub running: Running,
    pub values: Value,
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default)]
    pub diags: Vec<Diag>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub format: String,
    pub writable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_not: Option<WhyNot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_differs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_not_history: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WhyNot {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Running {
    pub rev: String,
    pub ok: bool,
    #[serde(default)]
    pub file_diags: Vec<Diag>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Runtime {
    #[serde(default)]
    pub sessions: std::collections::BTreeMap<String, SessionInfo>,
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub bound: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub restart_pending: Vec<String>,
    #[serde(default)]
    pub allows: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionInfo {
    pub secret_fp: String,
    #[serde(default)]
    pub from: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Subscribe {
    #[serde(default)]
    pub status_ms: u64,
    #[serde(default)]
    pub config: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Validate {
    pub base: String,
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Apply {
    pub base: String,
    pub ops: Vec<Op>,
    pub plan: String,
    #[serde(default)]
    pub confirm: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedSecret {
    pub handle: String,
    pub fp: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectInfo {
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Suggest {
    pub what: String,
    #[serde(default)]
    pub near: Option<u32>,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub proto: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub entity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Suggestions {
    pub values: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HistoryRequest {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub diff: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HistoryReply {
    pub entries: Vec<AuditEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<crate::plan::Diff>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at: u64,
    pub visitor: String,
    #[serde(default)]
    pub from: String,
    pub base: String,
    pub rev: String,
    #[serde(default)]
    pub changes: Vec<Change>,
    pub result: String,
    #[serde(default)]
    pub revertible: bool,
    #[serde(default)]
    pub details_kept: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Revert {
    pub to: String,
    pub base: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigEvent {
    pub rev: String,
    pub by: String,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Revoked {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockedOut {
    pub reasons: Vec<crate::plan::Refusal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvalidBody {
    pub diags: Vec<Diag>,
}

pub fn path(parts: &[&str]) -> Path {
    parts.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelopes() {
        let r = Incoming::parse(r#"{"re":7,"ok":false,"err":{"code":"conflict","message":"m","body":{"current":"b2s:1"}}}"#).unwrap();
        let Incoming::Reply {
            re: 7,
            result: Err(e),
        } = r
        else {
            panic!()
        };
        assert_eq!(e.code, ErrorCode::Conflict);
        let r = Incoming::parse(r#"{"re":1,"ok":false,"err":{"code":"brand-new","message":"m"}}"#)
            .unwrap();
        let Incoming::Reply { result: Err(e), .. } = r else {
            panic!()
        };
        assert_eq!(e.code, ErrorCode::Unknown);
        let ev = Incoming::parse(r#"{"ev":"later","body":{"x":1}}"#).unwrap();
        assert!(matches!(ev, Incoming::Event { .. }));
        assert_eq!(reply_ok(3, json!({}))["re"], 3);
        assert_eq!(
            serde_json::to_value(ErrorCode::LockedOut).unwrap(),
            json!("locked-out")
        );
        assert_eq!(ErrorCode::NotWritable.as_str(), "not-writable");
    }

    #[test]
    fn welcome_ignores_unknown_fields() {
        let w: Welcome = serde_json::from_value(json!({
            "server": {"version": "0.3.1", "future": 1},
            "protocol": 1,
            "you": {"visitor": "ops", "level": "write"},
            "features": ["status.stream"],
            "limits": {"max_message": 4194304, "max_inflight": 8},
            "schema_hash": "b2s:1",
            "extra": true
        }))
        .unwrap();
        assert!(w.has("status.stream"));
        assert_eq!(w.you.level, AdminLevel::Write);
    }
}
