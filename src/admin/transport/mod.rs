mod session;

use std::sync::OnceLock;
use std::time::Duration;

pub use session::{Access, run};

pub const PING: Duration = Duration::from_secs(25);
pub const SILENCE: Duration = Duration::from_secs(90);

static SHORTER: OnceLock<(Duration, Duration)> = OnceLock::new();

pub fn keepalive() -> (Duration, Duration) {
    SHORTER.get().copied().unwrap_or((PING, SILENCE))
}

#[doc(hidden)]
pub fn shorten_keepalive_for_tests(ping: Duration, silence: Duration) {
    let _ = SHORTER.set((ping, silence));
}
