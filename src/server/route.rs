use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use tracing::debug;

use super::CountGuard;
use super::pick::{Held, Hop, Picked};
use super::share::{FairShare, Pool};
use super::{Shared, take_channel};
use crate::config::settings::ServerSettings;
use crate::forward::{Early, ForwardError, Opened};
use crate::wire::protocol::Status;
use crate::wire::transport::Channel;

pub(super) struct Way<'a> {
    pub service: Option<&'a str>,
    pub label: &'a str,
    pub peer: SocketAddr,
    pub source: Option<IpAddr>,
    pub several: bool,
}

impl Way<'_> {
    pub(super) fn accepted(&self, shared: &Shared, hop: &Hop) {
        if let Some(service) = self.service.filter(|_| self.several) {
            shared.accepted(service, hop.name(), &hop.target);
        }
    }
}

pub(super) enum Outcome {
    Done {
        client: String,
        target: String,
        result: Result<(), ForwardError>,
    },
    Refused {
        client: String,
        target: String,
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
    clients: std::vec::IntoIter<Hop>,
    held: Option<Held>,
    early: Early,
    last: Outcome,
    active: Option<(CountGuard, Option<CountGuard>, Option<FairShare>)>,
}

impl<'a> Tries<'a> {
    pub(super) fn new(shared: &'a Arc<Shared>, way: &'a Way<'a>, picked: Picked) -> Tries<'a> {
        let (clients, held) = picked.split();
        Tries {
            shared,
            way,
            settings: shared.settings(),
            clients: clients.into_iter(),
            held,
            early: Early::default(),
            last: Outcome::Unavailable(Status::Offline),
            active: None,
        }
    }

    pub(super) async fn next(&mut self) -> Option<(Hop, Channel)> {
        while let Some(session) = self.clients.next() {
            self.active = None;
            if self.clients.len() == 0 {
                self.early.last_try();
            }
            let client = session.name();
            let limit = self
                .settings
                .clients
                .get(client)
                .and_then(|c| c.max_connections)
                .unwrap_or(self.settings.max_connections_per_client);
            let (active, entry, before) = match self.held.take() {
                Some(h) if h.name == client && h.target == session.target => {
                    (h.guard, h.entry, h.before)
                }
                _ => {
                    let (g, before) = self.shared.client_opens(client);
                    let entry = self
                        .way
                        .service
                        .map(|s| self.shared.entry_opens(s, &session));
                    (g, entry, before)
                }
            };
            if before >= limit {
                self.last = Outcome::Full(client.to_string());
                continue;
            }
            let pool = Pool::Client(client.to_string());
            let Ok(share) = self
                .shared
                .visitor_shares
                .take(pool, self.way.source, limit, before)
            else {
                debug!(peer = %self.way.peer, "{}: client \"{client}\" is over half full and this address holds its fair part", self.way.label);
                self.last = Outcome::Full(client.to_string());
                continue;
            };
            match take_channel(self.shared, &session, self.way.label, self.way.peer).await {
                Ok(ch) => {
                    self.active = Some((active, entry, share));
                    return Some((session, ch));
                }
                Err(status) => {
                    if self.way.several
                        && status != crate::wire::protocol::Status::Busy
                        && let Some(service) = self.way.service
                    {
                        self.shared.troubled(
                            service,
                            &session,
                            &session.target,
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

    pub(super) fn opened(&mut self, session: &Hop, opened: Opened) -> Option<Outcome> {
        let way = self.way;
        let client = session.name();
        let service = way.service.filter(|_| way.several);
        if let Opened::Done(Err(ForwardError::Status(Status::Busy)))
        | Opened::Refused(ForwardError::Status(Status::Busy)) = &opened
        {
            self.shared.metrics.client_busy(client);
        }
        match opened {
            Opened::Done(result) => Some(Outcome::Done {
                client: client.to_string(),
                target: session.target.clone(),
                result,
            }),
            Opened::Refused(error) => {
                if let Some(service) = service {
                    self.shared
                        .troubled(service, session, &session.target, &error);
                }
                let replayable =
                    matches!(error, ForwardError::Status(_)) || self.early.sent_nothing();
                if !replayable {
                    self.clients = Vec::new().into_iter();
                }
                debug!(peer = %way.peer, "{}: client \"{client}\" did not take the connection: {error:?}", way.label);
                let refusal = |e: &ForwardError| {
                    matches!(e, ForwardError::Status(Status::Denied | Status::Invalid))
                };
                let earlier_says_more = refusal(&error)
                    && matches!(&self.last, Outcome::Refused { error: earlier, .. } if !refusal(earlier));
                if !earlier_says_more {
                    self.last = Outcome::Refused {
                        client: client.to_string(),
                        target: session.target.clone(),
                        error,
                    };
                }
                None
            }
            Opened::Gone(e) => Some(Outcome::Gone(e)),
        }
    }

    pub(super) fn outcome(self) -> Outcome {
        self.last
    }
}
