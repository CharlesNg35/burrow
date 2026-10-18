use std::net::SocketAddr;
use std::sync::Arc;

use tracing::debug;

use super::CountGuard;
use super::{ClientSession, Shared, take_channel};
use crate::config::settings::ServerSettings;
use crate::forward::{Early, ForwardError, Opened};
use crate::handshake::Channel;
use crate::protocol::Status;

pub(super) struct Way<'a> {
    pub service: Option<&'a str>,
    pub target: &'a str,
    pub label: &'a str,
    pub peer: SocketAddr,
    pub several: bool,
}

impl Way<'_> {
    pub(super) fn accepted(&self, shared: &Shared, client: &str) {
        if let Some(service) = self.service.filter(|_| self.several) {
            shared.accepted(service, client, self.target);
        }
    }
}

pub(super) enum Outcome {
    Done {
        client: String,
        result: Result<(), ForwardError>,
    },
    Refused {
        client: String,
        error: ForwardError,
    },
    Full(String),
    Unavailable(Status),
    Gone(Option<std::io::Error>),
}

pub(super) struct Tries<'a> {
    shared: &'a Arc<Shared>,
    way: &'a Way<'a>,
    settings: Arc<ServerSettings>,
    clients: std::vec::IntoIter<Arc<ClientSession>>,
    early: Early,
    last: Outcome,
    active: Option<CountGuard>,
}

impl<'a> Tries<'a> {
    pub(super) fn new(
        shared: &'a Arc<Shared>,
        way: &'a Way<'a>,
        clients: Vec<Arc<ClientSession>>,
    ) -> Tries<'a> {
        Tries {
            shared,
            way,
            settings: shared.settings(),
            clients: clients.into_iter(),
            early: Early::default(),
            last: Outcome::Unavailable(Status::Offline),
            active: None,
        }
    }

    pub(super) async fn next(&mut self) -> Option<(Arc<ClientSession>, Channel)> {
        while let Some(session) = self.clients.next() {
            self.active = None;
            if self.clients.len() == 0 {
                self.early.last_try();
            }
            let client = &session.name;
            let limit = self
                .settings
                .clients
                .get(client)
                .and_then(|c| c.max_connections)
                .unwrap_or(self.settings.max_connections_per_client);
            let (active, before) = self.shared.client_opens(client);
            if before >= limit {
                self.last = Outcome::Full(client.clone());
                continue;
            }
            match take_channel(self.shared, &session, self.way.label, self.way.peer).await {
                Ok(ch) => {
                    self.active = Some(active);
                    return Some((session, ch));
                }
                Err(status) => {
                    if self.way.several
                        && status != crate::protocol::Status::Busy
                        && let Some(service) = self.way.service
                    {
                        self.shared.troubled(
                            service,
                            &session,
                            self.way.target,
                            &crate::forward::no_reply(),
                        );
                    }
                    self.last = Outcome::Unavailable(status);
                }
            }
        }
        None
    }

    pub(super) fn early(&mut self) -> &mut Early {
        &mut self.early
    }

    pub(super) fn opened(&mut self, session: &ClientSession, opened: Opened) -> Option<Outcome> {
        let way = self.way;
        let client = &session.name;
        let service = way.service.filter(|_| way.several);
        match opened {
            Opened::Done(result) => Some(Outcome::Done {
                client: client.clone(),
                result,
            }),
            Opened::Refused(error) => {
                if let Some(service) = service {
                    self.shared.troubled(service, session, way.target, &error);
                }
                let replayable =
                    matches!(error, ForwardError::Status(_)) || self.early.sent_nothing();
                if !replayable {
                    self.clients = Vec::new().into_iter();
                }
                debug!(peer = %way.peer, "{}: client \"{client}\" did not take the connection: {error:?}", way.label);
                self.last = Outcome::Refused {
                    client: client.clone(),
                    error,
                };
                None
            }
            Opened::Gone(e) => Some(Outcome::Gone(e)),
        }
    }

    pub(super) fn outcome(self) -> Outcome {
        self.last
    }
}
