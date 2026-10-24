use std::net::SocketAddr;
use std::sync::Arc;

use super::{Server, Shared};
use crate::base::failure::Failure;
use crate::config::settings::ServerSettings;

pub(super) enum WebTask {}

impl WebTask {
    pub(super) async fn bind(
        _: &Arc<Shared>,
        _: &ServerSettings,
    ) -> Result<Option<WebTask>, Failure> {
        Ok(None)
    }
}

impl Server {
    pub fn web_addr(&self) -> Option<SocketAddr> {
        self.web.as_ref().map(|t| match *t {})
    }

    pub fn web_http_addr(&self) -> Option<SocketAddr> {
        self.web.as_ref().map(|t| match *t {})
    }

    pub(super) fn log_web(&self) {}

    pub(super) async fn stop_web(&mut self) {}

    pub(super) async fn apply_web(&mut self, _: &mut ServerSettings, _: &[SocketAddr]) {}

    pub(super) async fn web_waiting(&mut self, _: &mut ServerSettings) -> bool {
        false
    }
}
