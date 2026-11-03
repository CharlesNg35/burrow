//! The wire layer: secrets, the Noise handshake, records, the protocol messages and the transports.
//! It may use base and the shared burrow-config enums; it must not depend on the config module, roles or ops.

pub mod fetch;
pub mod handshake;
pub mod noise;
pub mod protocol;
pub mod proxy;
pub mod proxy_protocol;
pub mod record;
pub mod secret;
pub mod transport;
