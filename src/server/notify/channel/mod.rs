mod email;
mod http;
mod ntfy;
mod webhook;

use std::future::Future;
use std::pin::Pin;

use super::Letter;
use crate::config::settings::NotifyVia;

pub(super) type Sent<'a> = Pin<Box<dyn Future<Output = Result<(), SendError>> + Send + 'a>>;

pub(super) trait Channel: Send + Sync {
    fn kind(&self) -> &'static str;
    fn send<'a>(&'a self, letter: &'a Letter) -> Sent<'a>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SendError {
    Retry(String),
    Permanent(String),
}

impl SendError {
    pub(super) fn retryable(&self) -> bool {
        matches!(self, SendError::Retry(_))
    }
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Retry(why) => f.write_str(why),
            SendError::Permanent(why) => {
                write!(f, "{why}; not tried again until the config changes")
            }
        }
    }
}

pub(super) fn open(via: &NotifyVia) -> Result<Box<dyn Channel>, SendError> {
    Ok(match via {
        NotifyVia::Webhook(url) => Box::new(webhook::Webhook::new(url)?),
        NotifyVia::Ntfy(url) => Box::new(ntfy::Ntfy::new(url)?),
        NotifyVia::Email(m) => Box::new(email::Email::new(m)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_permanent_failure_says_it_is_not_retried() {
        assert!(SendError::Retry("timed out".into()).retryable());
        let p = SendError::Permanent("the server answered 401 Unauthorized".into());
        assert!(!p.retryable());
        assert_eq!(
            p.to_string(),
            "the server answered 401 Unauthorized; not tried again until the config changes"
        );
    }

    #[test]
    fn each_destination_opens_as_its_own_kind() {
        let kind = |via| open(&via).map(|c| c.kind()).unwrap();
        assert_eq!(
            kind(NotifyVia::Webhook("https://h.example/x".into())),
            "webhook"
        );
        assert_eq!(kind(NotifyVia::Ntfy("https://ntfy.sh/t".into())), "ntfy");
    }
}

#[cfg(test)]
mod testing;
