//! The wire layer: secrets, the Noise handshake, records, the protocol messages and the transports.
//! It may use base only; it must not depend on config, roles or ops.

pub mod handshake;
pub mod noise;
pub mod protocol;
pub mod proxy;
pub mod proxy_protocol;
pub mod record;
pub mod secret;
pub mod transport;
