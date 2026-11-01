//! Small I/O helpers shared by the burrow tunnel and web proxy: lingering close for early HTTP/1 answers (and, later, in-memory pipes).
//! Depends on tokio, hyper and hyper-util only; never on burrow, burrow-web or the config and UI crates.

#![forbid(unsafe_code)]

pub mod linger;
