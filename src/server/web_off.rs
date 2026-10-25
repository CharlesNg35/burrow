use std::net::SocketAddr;
use std::sync::Arc;

use super::{Server, Shared};
use crate::base::failure::Failure;
use crate::config::settings::ServerSettings;
use crate::wire::transport::Channel;

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

    pub(super) async fn apply_web(
        &mut self,
        _: &mut ServerSettings,
        _: &[SocketAddr],
    ) -> Option<burrow_admin_proto::status::PortWait> {
        None
    }

    pub(super) async fn web_waiting(&mut self, _: &mut ServerSettings) -> bool {
        false
    }
}

pub(super) enum Http {}

pub(super) struct Checks;

impl Checks {
    pub(super) fn new() -> Checks {
        Checks
    }

    pub(super) fn refresh(&mut self, _: &Arc<ServerSettings>) {}

    pub(super) fn of(&self, _: &str) -> Option<Http> {
        None
    }
}

pub(super) async fn exchange(_: Channel, _: bool, http: &Http) -> Result<(), String> {
    match *http {}
}
