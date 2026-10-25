use std::time::Duration;

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
pub const REQUEST_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const RESPONSE_HEADER_TIMEOUT: Duration = Duration::from_secs(60);
