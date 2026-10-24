//! The burrow admin protocol: messages, frames, plans and the schema shared by the server and the admin UI.
//! It may depend only on serde, serde_json and schemars, never on burrow or the UI crates.

#![forbid(unsafe_code)]

pub mod diag;
pub mod frame;
pub mod msg;
pub mod ops;
pub mod plan;
pub mod schema;
pub mod status;
pub mod value;

pub use serde_json;

pub type Path = Vec<String>;

pub const PROTOCOL_MIN: u8 = 1;
pub const PROTOCOL_MAX: u8 = 1;
pub const SCHEMA_VERSION: u32 = 1;

pub mod features {
    pub const STATUS_STREAM: &str = "status.stream";
    pub const CONFIG_EVENTS: &str = "config.events";
    pub const SECRET_GENERATE: &str = "secret.generate";
    pub const CONNECT_SNIPPETS: &str = "connect.snippets";
    pub const SUGGEST_FREE_PORT: &str = "suggest.free-port";
    pub const HISTORY: &str = "history";
    pub const REVERT: &str = "revert";
    pub const OPS_RENAME: &str = "ops.rename";
    pub const SET_TEXT: &str = "ops.set-text";

    pub const ALL: &[&str] = &[
        STATUS_STREAM,
        CONFIG_EVENTS,
        SECRET_GENERATE,
        CONNECT_SNIPPETS,
        SUGGEST_FREE_PORT,
        HISTORY,
        REVERT,
        OPS_RENAME,
        SET_TEXT,
    ];
}
