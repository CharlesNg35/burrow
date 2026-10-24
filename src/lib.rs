//! burrow, a fast and secure reverse tunnel, as a library built in layers: base, wire, config, roles and ops.
//! Each layer imports only the layers below it; xtask/src/layers.rs lists the layer of every module.
//! It must not depend on the admin UI crates, which are WebAssembly only.

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
pub mod app;
pub mod base;
pub mod client;
pub mod config;
pub mod dialer;
pub mod edit;
pub mod forward;
pub mod init;
pub mod install;
pub mod instance;
pub mod metrics;
pub mod policy;
pub mod server;
pub mod service;
pub mod status;
pub mod udp;
pub mod update;
pub mod visitor;
pub mod wire;
