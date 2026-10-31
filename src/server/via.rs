use std::sync::Arc;

use super::local::SelfDialer;
use super::{ClientSession, Shared, TakeError};
use crate::wire::protocol::TOKEN_LEN;
use crate::wire::transport::Channel;
use burrow_config::SELF;

#[derive(Clone)]
pub(crate) enum Via {
    Client(Arc<ClientSession>),
    Local(Arc<SelfDialer>),
}

impl Via {
    pub(crate) fn name(&self) -> &str {
        match self {
            Via::Client(s) => &s.name,
            Via::Local(_) => SELF,
        }
    }

    pub(crate) fn token(&self) -> [u8; TOKEN_LEN] {
        match self {
            Via::Client(s) => s.token,
            Via::Local(d) => d.token(),
        }
    }

    pub(crate) async fn take(
        &self,
        wait: std::time::Duration,
        source: Option<std::net::IpAddr>,
    ) -> Result<Channel, TakeError> {
        match self {
            Via::Client(s) => s.take(wait, source).await,
            Via::Local(d) => Ok(d.open()),
        }
    }

    pub(crate) async fn closed(&self) {
        match self {
            Via::Client(s) => s.closed().await,
            Via::Local(_) => std::future::pending().await,
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        match self {
            Via::Client(s) => s.is_closed(),
            Via::Local(_) => false,
        }
    }

    pub(crate) fn allow(&self) -> Option<Vec<String>> {
        match self {
            Via::Client(s) => s.allow(),
            Via::Local(d) => Some(d.allow()),
        }
    }

    pub(crate) fn admitted(&self, shared: &Shared) -> bool {
        match self {
            Via::Client(s) => shared.allows(&s.name, &s.secret),
            Via::Local(_) => true,
        }
    }
}

impl Shared {
    pub(crate) fn via(&self, name: &str) -> Option<Via> {
        self.connected(std::slice::from_ref(&name.to_string()))
            .into_iter()
            .next()
    }

    pub(crate) fn connected(&self, names: &[String]) -> Vec<Via> {
        let sessions = self.sessions.lock().expect("sessions lock");
        names
            .iter()
            .filter_map(|name| match name.as_str() {
                SELF => Some(Via::Local(self.local.clone())),
                _ => sessions.get(name).cloned().map(Via::Client),
            })
            .collect()
    }
}
