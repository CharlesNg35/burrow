use ::http::header::CONTENT_TYPE;
use burrow_web::tls::acme::http::Client;

use super::{Channel, SendError, Sent, http};
use crate::server::notify::Letter;

pub(super) struct Webhook {
    client: Client,
    url: String,
}

impl Webhook {
    pub(super) fn new(url: &str) -> Result<Webhook, SendError> {
        Ok(Webhook {
            client: http::client()?,
            url: url.to_string(),
        })
    }
}

pub(super) fn body(letter: &Letter) -> serde_json::Value {
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

impl Channel for Webhook {
    fn kind(&self) -> &'static str {
        "webhook"
    }

    fn send<'a>(&'a self, letter: &'a Letter) -> Sent<'a> {
        Box::pin(async move {
            let req = http::request(&self.url)
                .header(CONTENT_TYPE, "application/json")
                .body(body(letter).to_string().into_bytes());
            http::post(&self.client, req).await
        })
    }
}

#[cfg(test)]
mod tests;
