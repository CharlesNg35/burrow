mod live;
mod publish;
pub mod query;
pub mod render;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use publish::{Published, publish, publish_in};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub config: PathBuf,
    pub pid: u32,
    pub port: u16,
    pub token: String,
}
