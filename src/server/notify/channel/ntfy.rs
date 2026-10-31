use ::http::Request;
use burrow_web::tls::acme::http::Client;

use super::{Channel, SendError, Sent, http};
use crate::server::notify::Letter;

pub(super) struct Ntfy {
    client: Client,
    url: String,
}

impl Ntfy {
    pub(super) fn new(url: &str) -> Result<Ntfy, SendError> {
        Ok(Ntfy {
            client: http::client()?,
            url: url.to_string(),
        })
    }
}

fn ascii(text: &str) -> String {
    text.chars()
        .map(|c| match c.is_ascii() && !c.is_ascii_control() {
            true => c,
            false => '?',
        })
        .collect()
}

pub(super) fn request(url: &str, letter: &Letter) -> Result<Request<Vec<u8>>, ::http::Error> {
    let (priority, tags) = match letter.message.resolved {
        true => ("default", "white_check_mark"),
        false => ("high", "warning"),
    };
    http::request(url)
        .header("Title", ascii(&letter.title()))
        .header("Priority", priority)
        .header("Tags", tags)
        .body(letter.message.text.clone().into_bytes())
}

impl Channel for Ntfy {
    fn kind(&self) -> &'static str {
        "ntfy"
    }

    fn send<'a>(&'a self, letter: &'a Letter) -> Sent<'a> {
        Box::pin(async move { http::post(&self.client, request(&self.url, letter)).await })
    }
}

#[cfg(test)]
mod tests;
