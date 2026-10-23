use std::time::Duration;

use crate::config::settings::VisitorSettings;
use crate::dialer::{DialError, Dialer};
use crate::failure::Failure;
use crate::protocol::{
    ChannelKind, GRANTS_REQUEST, GRANTS_REQUEST_V2, Grants, Hello, Status, VERSION,
};

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

pub struct Fetched {
    pub grants: Grants,
    pub tcp_only: bool,
}

pub const OLD_SERVER_NOTE: &str = "the server is older than v0.1.2, so it listed its TCP services only; UDP forwards were kept as they are";

pub fn grants(settings: &VisitorSettings) -> Result<Fetched, NotAsked> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| NotAsked {
            failure: Failure::new(format!("cannot start the async runtime: {e}")),
            rejected: false,
        })?;
    rt.block_on(ask(&Dialer::visitor(settings)))
}

pub async fn ask(dialer: &Dialer) -> Result<Fetched, NotAsked> {
    match once(dialer, &GRANTS_REQUEST_V2).await? {
        Answer::Grants(grants) => Ok(Fetched {
            grants,
            tcp_only: false,
        }),
        Answer::Status(Some(Status::Invalid)) => match once(dialer, &GRANTS_REQUEST).await? {
            Answer::Grants(grants) => Ok(Fetched {
                grants,
                tcp_only: true,
            }),
            Answer::Status(status) => Err(refusal(status, dialer)),
        },
        Answer::Status(status) => Err(refusal(status, dialer)),
    }
}

enum Answer {
    Grants(Grants),
    Status(Option<Status>),
}

fn refusal(status: Option<Status>, dialer: &Dialer) -> NotAsked {
    NotAsked {
        failure: refused(status, &dialer.server, &dialer.name),
        rejected: status == Some(Status::Forbidden),
    }
}

async fn once(dialer: &Dialer, request: &[u8]) -> Result<Answer, NotAsked> {
    let answered = |failure: Failure| NotAsked {
        failure,
        rejected: false,
    };
    let (server, name) = (&dialer.server, &dialer.name);
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
        .send(&mut ch.stream, request)
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
        Some(b) if matches!(b.first(), Some(0x22 | 0x25)) => {
            Grants::decode(b).map(Answer::Grants).map_err(|_| {
                answered(Failure::new(format!(
                    "the server {server} sent an answer burrow cannot read"
                )))
            })
        }
        Some(b) => Ok(Answer::Status(Status::decode(b).ok())),
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
