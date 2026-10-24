//! The value syntaxes of burrow configs: addresses, ports, targets, rules, rates and durations.
//! It must not depend on any other crate.

#![forbid(unsafe_code)]

pub mod address;
pub mod cidr;
pub mod duration;
pub mod endpoint;
pub mod env;
pub mod grant;
pub mod hostport;
pub mod ip;
pub mod name;
pub mod ports;
pub mod rate;
pub mod rule;
pub mod secret_fmt;
pub mod target;
pub mod unix;
pub mod web;
