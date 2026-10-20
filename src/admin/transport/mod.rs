mod session;

use std::sync::OnceLock;
use std::time::Duration;

pub use session::{Access, run};

/// Both ends of an admin session ping after 25 s and close after 90 s
/// without hearing from the other side.
pub const PING: Duration = Duration::from_secs(25);
pub const SILENCE: Duration = Duration::from_secs(90);

static SHORTER: OnceLock<(Duration, Duration)> = OnceLock::new();

/// The ping interval and the silence limit of admin sessions.
pub fn keepalive() -> (Duration, Duration) {
    SHORTER.get().copied().unwrap_or((PING, SILENCE))
}

/// Shortens both timers for the whole process; only tests call this.
#[doc(hidden)]
pub fn shorten_keepalive_for_tests(ping: Duration, silence: Duration) {
    let _ = SHORTER.set((ping, silence));
}
