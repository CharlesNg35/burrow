use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::settings::{Forward, VisitorSettings};
use crate::dialer::{DialError, Dialer};
use crate::forward::{ForwardError, visit_tcp};
use crate::protocol::{ChannelKind, Hello, Status, VERSION, VisitRequest, VisitTarget};
use crate::throttle::Throttle;
use crate::transport::tcp::tune;

pub(super) struct Incoming {
    name: String,
    to: VisitTarget,
    stream: TcpStream,
    peer: SocketAddr,
}

pub(super) async fn accept(
    name: String,
    forward: Forward,
    listener: TcpListener,
    tx: mpsc::Sender<Incoming>,
) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!("forward \"{name}\": accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let conn = Incoming {
            name: name.clone(),
            to: forward.to.clone(),
            stream,
            peer,
        };
        if tx.send(conn).await.is_err() {
            return;
        }
    }
}

fn rejected(dialer: &Dialer) -> String {
    format!(
        "the server {} rejected visitor \"{}\": wrong secret or unknown visitor name, or the server is older than burrow 0.2.0 (check both against the server config, or print a new visitor config on the server with: burrow visitor-config {} -o visitor.yaml)",
        dialer.server, dialer.name, dialer.name
    )
}

fn explain(status: Status, to: &VisitTarget, visitor: &str) -> String {
    let goal = match to {
        VisitTarget::Service(name) => format!("service \"{name}\""),
        VisitTarget::Client { target, .. } => target.clone(),
    };
    match (status, to) {
        (Status::Forbidden, VisitTarget::Service(name)) => format!(
            "the server does not let visitor \"{visitor}\" use service \"{name}\": it does not exist, or does not list \"{visitor}\" in its \"visitors\""
        ),
        (Status::Forbidden, VisitTarget::Client { client, .. }) => format!(
            "the server does not let visitor \"{visitor}\" reach client \"{client}\" directly: that needs \"reach: {{ {client}: any }}\" for \"{visitor}\" in the server config"
        ),
        (Status::Offline, VisitTarget::Service(name)) => {
            format!("the client behind service \"{name}\" is not connected to the server")
        }
        (Status::Offline, VisitTarget::Client { client, .. }) => {
            format!("client \"{client}\" is not connected to the server")
        }
        (Status::Busy, _) => {
            "the server is at a connection limit for this visitor, service or client".into()
        }
        (Status::Denied, _) => {
            format!("the client refused {goal}: its target is not in the client's allow list")
        }
        (Status::Unreachable, _) => {
            format!("the client could not connect to {goal} (is it running?)")
        }
        (Status::Unresolved, _) => format!("the client could not resolve the host of {goal}"),
        (Status::Invalid, _) => format!("the request for {goal} was rejected as invalid"),
        (Status::Ok, _) => String::new(),
    }
}

pub(super) async fn check(dialer: Arc<Dialer>, forwards: Vec<(String, VisitTarget)>) {
    let mut ready = true;
    for (name, to) in forwards {
        let ch = match dialer
            .open(ChannelKind::Visit, Hello::Visit { version: VERSION })
            .await
        {
            Ok(ch) => ch,
            Err(DialError::Rejected) => {
                warn!("{}", rejected(&dialer));
                return;
            }
            Err(DialError::Lost(why)) => {
                warn!("{why}; the forwards will work once the server is reachable");
                return;
            }
        };
        let request = VisitRequest { check: true, to };
        match ask(ch, &request).await {
            Some(Status::Ok) => {}
            Some(st) => {
                ready = false;
                warn!(
                    "forward \"{name}\": {}",
                    explain(st, &request.to, &dialer.name)
                );
            }
            None => {
                ready = false;
                warn!(
                    "forward \"{name}\": the server did not answer the check; it may be older than burrow 0.2.0"
                );
            }
        }
    }
    if ready {
        info!(
            "connected to {} as visitor \"{}\"",
            dialer.server, dialer.name
        );
    }
}

async fn ask(mut ch: crate::handshake::Channel, request: &VisitRequest) -> Option<Status> {
    ch.writer
        .send(&mut ch.stream, &request.encode())
        .await
        .ok()?;
    let answer = tokio::time::timeout(Duration::from_secs(10), ch.reader.recv(&mut ch.stream));
    match answer.await {
        Ok(Ok(Some(b))) => Status::decode(b).ok(),
        _ => None,
    }
}

pub(super) async fn visit(dialer: Arc<Dialer>, settings: Arc<VisitorSettings>, conn: Incoming) {
    let Incoming {
        name,
        to,
        stream,
        peer,
    } = conn;
    tune(&stream, &settings.tcp);
    let ch = match dialer
        .open(ChannelKind::Visit, Hello::Visit { version: VERSION })
        .await
    {
        Ok(ch) => ch,
        Err(e) => {
            static LOST: Throttle = Throttle::new();
            if let Some(more) = LOST.allow() {
                let why = match e {
                    DialError::Rejected => rejected(&dialer),
                    DialError::Lost(why) => why,
                };
                warn!("forward \"{name}\": {why}{more}");
            }
            return;
        }
    };
    let request = VisitRequest {
        check: false,
        to: to.clone(),
    }
    .encode();
    match visit_tcp(ch, stream, &request).await {
        Ok(()) => debug!(%peer, "forward \"{name}\": connection closed"),
        Err(ForwardError::Status(st)) => {
            static REFUSED: Throttle = Throttle::new();
            if let Some(more) = REFUSED.allow() {
                warn!(
                    "forward \"{name}\": {}{more}",
                    explain(st, &to, &dialer.name)
                );
            }
        }
        Err(ForwardError::Protocol) => {
            warn!("forward \"{name}\": the server sent an invalid reply")
        }
        Err(ForwardError::Io(e)) => debug!(%peer, "forward \"{name}\": connection ended: {e}"),
    }
}
