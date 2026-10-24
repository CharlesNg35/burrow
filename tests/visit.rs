mod common;
#[path = "visit/requests.rs"]
mod requests;
#[path = "visit/tcp.rs"]
mod tcp;
#[path = "visit/udp.rs"]
mod udp;
#[path = "visit/udp_forward.rs"]
mod udp_forward;

use std::net::SocketAddr;
use std::time::Duration;

use burrow::config::settings::ServerSettings;
use burrow::dialer::{DialError, Dialer};
use burrow::wire::protocol::{ChannelKind, Hello, Status, VisitRequest, VisitTarget};
use burrow::wire::secret::Secret;

use common::*;

struct Setup {
    tunnel: Tunnel,
    visitor_secret: Secret,
    echo: SocketAddr,
}

async fn setup(change: impl FnOnce(&mut ServerSettings, SocketAddr)) -> Setup {
    setup_with(change, |c| c).await
}

async fn setup_with(
    change: impl FnOnce(&mut ServerSettings, SocketAddr),
    client: impl FnOnce(
        burrow::config::settings::ClientSettings,
    ) -> burrow::config::settings::ClientSettings,
) -> Setup {
    let echo = echo_server().await;
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server
        .visitors
        .insert("laptop".into(), visitor_auth(visitor_secret.clone(), &[]));
    server
        .services
        .insert("db".into(), private_service(echo, &["laptop"]));
    change(&mut server, echo);
    let tunnel = tunnel_with(server, vec![], client, secret).await;
    Setup {
        tunnel,
        visitor_secret,
        echo,
    }
}

impl Setup {
    fn visitor(&self, forwards: Vec<(&str, VisitTarget)>) -> RunningVisitor {
        start_visitor(visitor_settings(
            self.tunnel.server.addr,
            self.visitor_secret.clone(),
            forwards,
        ))
    }

    fn reach(&self) -> VisitTarget {
        VisitTarget::Client {
            client: "home".into(),
            target: self.echo.to_string(),
        }
    }

    async fn ask(&self, check: bool, to: VisitTarget) -> Result<Status, &'static str> {
        ask(
            self.tunnel.server.addr,
            self.visitor_secret.clone(),
            check,
            to,
        )
        .await
    }
}

async fn ask(
    server: SocketAddr,
    secret: Secret,
    check: bool,
    to: VisitTarget,
) -> Result<Status, &'static str> {
    let dialer = Dialer::visitor(&visitor_settings(server, secret, vec![]));
    let mut ch = match dialer
        .open(ChannelKind::Visit, Hello::Visit { version: 1 })
        .await
    {
        Ok(ch) => ch,
        Err(DialError::Rejected) => return Err("rejected"),
        Err(DialError::Lost(_) | DialError::Unreachable(_)) => return Err("lost"),
    };
    ch.writer
        .send(
            &mut ch.stream,
            &VisitRequest {
                check,
                protocol: burrow_config::Protocol::Tcp,
                to,
            }
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
    match tokio::time::timeout(Duration::from_secs(10), ch.reader.recv(&mut ch.stream)).await {
        Ok(Ok(Some(b))) => Ok(Status::decode(b).unwrap()),
        _ => Err("no answer"),
    }
}

async fn refused(addr: SocketAddr) -> bool {
    let payload = pattern(1000, 3);
    try_roundtrip(addr, &payload).await.as_deref() != Some(&payload[..])
}
