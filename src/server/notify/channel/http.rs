use std::time::Duration;

use burrow_web::tls::acme::http::{Client, Limits};
use http::header::USER_AGENT;
use http::{Request, StatusCode};

use super::SendError;

pub(super) const TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn agent() -> String {
    format!("burrow/{}", env!("CARGO_PKG_VERSION"))
}

pub(super) fn client() -> Result<Client, SendError> {
    let limits = Limits {
        connect: Duration::from_secs(10),
        request: TIMEOUT,
        body: 64 * 1024,
    };
    Client::new(None, limits)
        .map(Client::with_plain_private)
        .map_err(SendError::Permanent)
}

pub(super) fn request(url: &str) -> http::request::Builder {
    Request::post(url).header(USER_AGENT, agent())
}

pub(super) fn verdict(status: StatusCode) -> Result<(), SendError> {
    let why = format!("the server answered {status}");
    match status {
        s if s.is_success() => Ok(()),
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS => Err(SendError::Retry(why)),
        s if s.is_server_error() => Err(SendError::Retry(why)),
        s if s.is_client_error() => Err(SendError::Permanent(why)),
        _ => Err(SendError::Retry(why)),
    }
}

pub(super) async fn post(
    client: &Client,
    req: Result<Request<Vec<u8>>, http::Error>,
) -> Result<(), SendError> {
    let req = req.map_err(|e| SendError::Permanent(e.to_string()))?;
    let resp = client.fetch(req).await.map_err(SendError::Retry)?;
    verdict(resp.status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_and_broken_servers_are_tried_again_and_refusals_are_not() {
        assert_eq!(verdict(StatusCode::NO_CONTENT), Ok(()));
        for retry in [500, 502, 503, 408, 429] {
            let s = StatusCode::from_u16(retry).unwrap();
            assert!(verdict(s).unwrap_err().retryable(), "{retry}");
        }
        for stop in [400, 401, 403, 404, 413] {
            let s = StatusCode::from_u16(stop).unwrap();
            assert!(!verdict(s).unwrap_err().retryable(), "{stop}");
        }
    }
}
