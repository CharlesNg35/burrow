//! Core tunnel tests: TCP and UDP traffic through the tunnel, connection handling and edge cases.

#[path = "integration/basics.rs"]
mod basics;
mod common;
#[path = "integration/connections.rs"]
mod connections;
#[path = "integration/edge_cases.rs"]
mod edge_cases;
