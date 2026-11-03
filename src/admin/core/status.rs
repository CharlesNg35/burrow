use crate::metrics::Status;

use super::Core;

impl Core {
    pub fn status(&self) -> Status {
        let mut status = self.live.status();
        let mut allows = self.live.allows();
        for c in status
            .clients
            .iter_mut()
            .filter(|c| c.connected && !c.local)
        {
            c.allow = allows.remove(&c.name);
        }
        let latest = self.live.latest();
        status.latest = (latest != Default::default()).then_some(latest);
        super::updates::offer(&mut status);
        status
    }
}
