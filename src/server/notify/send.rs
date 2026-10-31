use std::time::Duration;

use burrow_web::tls::acme::http::{Client, Limits};
use http::header::{CONTENT_TYPE, USER_AGENT};
use http::{Method, Request};
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message as Mail, Tokio1Executor};

use super::Letter;
use crate::config::settings::{EmailSettings, NotifyVia, SmtpSecurity};

const TIMEOUT: Duration = Duration::from_secs(30);

pub(super) enum Sender {
    Http {
        client: Client,
        ntfy: bool,
        url: String,
    },
    Email {
        mail: Box<EmailSettings>,
        smtp: AsyncSmtpTransport<Tokio1Executor>,
    },
}

fn agent() -> String {
    format!("burrow/{}", env!("CARGO_PKG_VERSION"))
}

impl Sender {
    pub(super) fn new(via: &NotifyVia) -> Result<Sender, String> {
        let http = |url: &str, ntfy: bool| -> Result<Sender, String> {
            let limits = Limits {
                connect: Duration::from_secs(10),
                request: TIMEOUT,
                body: 64 * 1024,
            };
            Ok(Sender::Http {
                client: Client::new(None, limits)?.with_plain_private(),
                ntfy,
                url: url.to_string(),
            })
        };
        match via {
            NotifyVia::Webhook(url) => http(url, false),
            NotifyVia::Ntfy(url) => http(url, true),
            NotifyVia::Email(m) => Ok(Sender::Email {
                smtp: transport(m)?,
                mail: Box::new(m.clone()),
            }),
        }
    }

    pub(super) async fn send(&self, letter: &Letter) -> Result<(), String> {
        match self {
            Sender::Http { client, ntfy, url } => {
                let req = match ntfy {
                    true => ntfy_request(url, letter),
                    false => webhook_request(url, letter),
                }
                .map_err(|e| e.to_string())?;
                let resp = client.fetch(req).await?;
                match resp.status() {
                    s if s.is_success() => Ok(()),
                    s => Err(format!("the server answered {s}")),
                }
            }
            Sender::Email { mail, smtp } => {
                let m = email(mail, letter)?;
                smtp.send(m).await.map(|_| ()).map_err(|e| e.to_string())
            }
        }
    }
}

pub(super) fn webhook_body(letter: &Letter) -> serde_json::Value {
    serde_json::json!({
        "event": letter.message.event.wire(),
        "resolved": letter.message.resolved,
        "kind": letter.message.kind.wire(),
        "name": letter.message.name,
        "title": letter.title(),
        "text": letter.message.text,
        "server": letter.server,
        "at": letter.at,
    })
}

fn webhook_request(url: &str, letter: &Letter) -> Result<Request<Vec<u8>>, http::Error> {
    Request::builder()
        .method(Method::POST)
        .uri(url)
        .header(USER_AGENT, agent())
        .header(CONTENT_TYPE, "application/json")
        .body(webhook_body(letter).to_string().into_bytes())
}

fn ascii(text: &str) -> String {
    text.chars()
        .map(|c| match c.is_ascii() && !c.is_ascii_control() {
            true => c,
            false => '?',
        })
        .collect()
}

pub(super) fn ntfy_request(url: &str, letter: &Letter) -> Result<Request<Vec<u8>>, http::Error> {
    let (priority, tags) = match letter.message.resolved {
        true => ("default", "white_check_mark"),
        false => ("high", "warning"),
    };
    Request::builder()
        .method(Method::POST)
        .uri(url)
        .header(USER_AGENT, agent())
        .header("Title", ascii(&letter.title()))
        .header("Priority", priority)
        .header("Tags", tags)
        .body(letter.message.text.clone().into_bytes())
}

fn transport(m: &EmailSettings) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let tls = || TlsParameters::new(m.host.clone()).map_err(|e| e.to_string());
    let security = match m.security {
        SmtpSecurity::Tls => Tls::Wrapper(tls()?),
        SmtpSecurity::StartTls => Tls::Required(tls()?),
        SmtpSecurity::StartTlsIfOffered => Tls::Opportunistic(tls()?),
    };
    let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(m.host.as_str())
        .port(m.port)
        .tls(security)
        .timeout(Some(TIMEOUT));
    if let (Some(user), Some(password)) = (&m.user, &m.password) {
        b = b.credentials(Credentials::new(
            user.clone(),
            password.expose().to_string(),
        ));
    }
    Ok(b.build())
}

fn email(m: &EmailSettings, letter: &Letter) -> Result<Mail, String> {
    let address = |a: &str| {
        a.parse::<lettre::message::Mailbox>()
            .map_err(|e| format!("{a}: {e}"))
    };
    Mail::builder()
        .from(address(&m.from)?)
        .to(address(&m.to)?)
        .subject(letter.title())
        .header(ContentType::TEXT_PLAIN)
        .body(letter.message.text.clone())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
