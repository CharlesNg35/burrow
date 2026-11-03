use super::{Health, own_status};
use crate::install::Manifest;

pub struct Connected;

impl Health for Connected {
    fn before(&self, _m: &Manifest) {}

    fn probe(&self, m: &Manifest, version: &str) -> Result<(), String> {
        let up = own_status(m)
            .and_then(|s| s.client)
            .is_some_and(|c| c.connected && c.build.version == version);
        if up {
            Ok(())
        } else {
            Err(format!("{version} did not connect to the server"))
        }
    }

    fn note(&self, _m: &Manifest, _version: &str) -> String {
        String::new()
    }
}
