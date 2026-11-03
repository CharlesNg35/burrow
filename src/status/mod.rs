//! `burrow status` and `burrow clients`: find running copies, read their status and show it, and ask a server to update its clients.

mod call;
pub mod clients;
mod live;
pub mod query;
pub mod render;
pub mod watch;
