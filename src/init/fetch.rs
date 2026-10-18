use std::time::Duration;

use crate::config::settings::VisitorSettings;
use crate::dialer::{DialError, Dialer};
use crate::failure::Failure;
use crate::protocol::{ChannelKind, GRANTS_REQUEST, Grants, Hello, Status, VERSION};

const ANSWER: Duration = Duration::from_secs(10);

pub struct NotAsked {
    pub failure: Failure,
    pub rejected: bool,
}

impl From<NotAsked> for Failure {
    fn from(n: NotAsked) -> Failure {
        n.failure
    }
}

pub fn grants(settings: &VisitorSettings) -> Result<Grants, NotAsked> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| NotAsked {
            failure: Failure::new(format!("cannot start the async runtime: {e}")),
            rejected: false,
        })?;
    rt.block_on(ask(settings))
}

async fn ask(settings: &VisitorSettings) -> Result<Grants, NotAsked> {
    let answered = |failure: Failure| NotAsked {
        failure,
        rejected: false,
    };
    let dialer = Dialer::visitor(settings);
    let (server, name) = (&settings.server, &settings.name);
    let mut ch = dialer
        .open(ChannelKind::Visit, Hello::Visit { version: VERSION })
        .await
        .map_err(|e| match e {
            DialError::Rejected => NotAsked {
                failure: Failure::new(format!("the server {server} rejected visitor \"{name}\""))
                    .hint("check the name and secret against the server config"),
                rejected: true,
            },
            DialError::Lost(why) | DialError::Unreachable(why) => NotAsked {
                failure: Failure::new(format!("cannot ask the server: {why}")),
                rejected: false,
            },
        })?;
    let lost = |e: std::io::Error| {
        answered(Failure::new(format!(
            "the server {server} closed the connection: {e}"
        )))
    };
    ch.writer
        .send(&mut ch.stream, &GRANTS_REQUEST)
        .await
        .map_err(lost)?;
    let answer = tokio::time::timeout(ANSWER, ch.reader.recv(&mut ch.stream))
        .await
        .map_err(|_| {
            answered(Failure::new(format!(
                "the server {server} did not answer in time"
            )))
        })?
        .map_err(lost)?;
    match answer {
        Some(b) if b.first() == Some(&0x22) => Grants::decode(b).map_err(|_| {
            answered(Failure::new(format!(
                "the server {server} sent an answer burrow cannot read"
            )))
        }),
        Some(b) => {
            let status = Status::decode(b).ok();
            Err(NotAsked {
                failure: refused(status, server, name),
                rejected: status == Some(Status::Forbidden),
            })
        }
        None => Err(answered(Failure::new(format!(
            "the server {server} closed the connection without answering"
        )))),
    }
}

fn refused(status: Option<Status>, server: &str, name: &str) -> Failure {
    match status {
        Some(Status::Forbidden) => Failure::new(format!(
            "the server {server} does not know visitor \"{name}\" any more"
        ))
        .hint("ask the server's admin to check the visitors in its config"),
        _ => Failure::new(format!(
            "the server {server} could not answer what visitor \"{name}\" may use"
        ))
        .hint(format!(
            "its log says why; or ask its admin for a ready config: {}",
            crate::failure::cmd(&format!("visitor-config {name} -o visitor.yaml"))
        )),
    }
}
