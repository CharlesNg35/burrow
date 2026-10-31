mod access;
pub mod compress;
pub mod forwarded;
pub mod h2;
pub mod headers;
pub mod health;
pub mod host;
mod idle;
pub mod limit;
pub mod logged;
pub mod pages;
mod pipeline;
pub mod redirect;
mod serve;
pub mod site;
mod stages;
mod stall;
pub mod upstream;

pub(crate) use serve::{Conn, Proto, serve};
pub use stall::STALL;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type Body = BoxBody<Bytes, BoxError>;

pub fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into())
        .map_err(|never| match never {})
        .boxed()
}

pub const MAX_HEAD: usize = 32 * 1024;
pub const MAX_HEADERS: usize = 100;
