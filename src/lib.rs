#![forbid(unsafe_code)]

extern crate self as burrow;

#[cfg(all(
    target_env = "musl",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )
))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(target_env = "musl", target_arch = "arm"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(feature = "admin-test-hooks", not(debug_assertions)))]
compile_error!("admin-test-hooks is for end-to-end tests only and never goes into a release build");

pub mod admin;
pub mod admin_local;
pub mod allowlist;
pub mod cli;
pub mod client;
pub mod config;
pub mod diagnostics;
pub mod dialer;
pub mod edit;
pub mod failure;
pub mod forward;
pub mod handshake;
pub mod init;
pub mod instance;
pub mod logfile;
pub mod metrics;
pub mod noise;
pub mod prompt;
pub mod protocol;
pub mod proxy;
pub mod proxy_protocol;
pub mod ratelimit;
pub mod record;
pub mod reload;
pub mod run;
pub mod secret;
pub mod server;
pub mod service;
pub mod status;
pub mod suggest;
pub mod throttle;
pub mod transport;
pub mod udp;
pub mod update;
pub mod validate;
pub mod visitor;
