use std::borrow::Cow;

use instant_acme::CertificateIdentifier;

use super::Problemish;
use super::group::Group;
use super::order::Failed;

#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    PlainNow,
    OtherCheck,
    Refused,
    Stop,
}

pub fn after(with_replaces: bool, f: &Failed) -> Next {
    match f.kind {
        Problemish::AlreadyReplaced | Problemish::Unsupported if with_replaces => Next::PlainNow,
        _ if f.validation => Next::OtherCheck,
        Problemish::Other if with_replaces && f.refused => Next::Refused,
        _ => Next::Stop,
    }
}

pub fn replaces(g: &Group, now: u64) -> Option<CertificateIdentifier<'static>> {
    g.held
        .as_ref()
        .filter(|h| h.names.iter().any(|n| g.names.contains(n)) && h.not_after > now)
        .filter(|_| g.planned.is_some_and(|p| p <= now) && g.replaces_refused < PLAIN_AFTER)
        .and_then(|h| h.ari_id.as_deref())
        .and_then(cert_id)
}

pub fn cert_id(ari_id: &str) -> Option<CertificateIdentifier<'static>> {
    let (aki, serial) = ari_id.split_once('.')?;
    Some(CertificateIdentifier {
        authority_key_identifier: Cow::Owned(aki.to_string()),
        serial: Cow::Owned(serial.to_string()),
    })
}

const PLAIN_AFTER: u8 = 2;
const ORDER_PLAIN: &str = "the certificate service refused the order that replaces the current certificate twice, so the next try orders a new one without replacing it";

pub fn refused_text(text: String, refusals: u8) -> String {
    match refusals == PLAIN_AFTER {
        true => format!("{text}; {ORDER_PLAIN}"),
        false => text,
    }
}
