use std::net::SocketAddr;

use base64::Engine as _;
use hickory_proto::op::{Message, ResponseCode, update_message};
use hickory_proto::rr::rdata::TXT;
use hickory_proto::rr::rdata::tsig::{TsigAlgorithm, TsigError};
use hickory_proto::rr::{Name, RData, RecordSet, RecordType, TSigVerifier, TSigner};
use zeroize::Zeroizing;

use super::wire::{self, UPDATE_TIMEOUT, shown};
use super::{Fut, Provider, Refusal, Secret, TXT_TTL};
use crate::tls::now_secs;

pub const FUDGE: u16 = 300;

pub struct Rfc2136 {
    server: String,
    key_name: Name,
    secret: Secret,
}

pub fn algorithm() -> TsigAlgorithm {
    TsigAlgorithm::HmacSha256
}

pub fn decode_secret(secret: &Secret) -> Result<Zeroizing<Vec<u8>>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(secret.expose().trim())
        .map(Zeroizing::new)
        .map_err(|_| "the rfc2136 secret is not base64".to_string())
}

impl Rfc2136 {
    pub fn new(server: &str, key_name: &str, secret: Secret) -> Result<Rfc2136, String> {
        decode_secret(&secret)?;
        Ok(Rfc2136 {
            server: server.trim().to_string(),
            key_name: wire::name(key_name)?.to_lowercase(),
            secret,
        })
    }

    async fn addr(&self) -> Result<SocketAddr, String> {
        tokio::net::lookup_host(self.server.as_str())
            .await
            .map_err(|e| format!("cannot find {}: {e}", self.server))?
            .next()
            .ok_or_else(|| format!("{} has no address", self.server))
    }

    fn signer(&self) -> Result<TSigner, String> {
        let key = decode_secret(&self.secret)?;
        TSigner::new(key.to_vec(), algorithm(), self.key_name.clone(), FUDGE)
            .map_err(|e| format!("cannot use the TSIG key: {e}"))
    }

    async fn update(&self, fqdn: &Name, value: &str, add: bool) -> Result<(), Refusal> {
        let addr = self.addr().await?;
        let zone = super::lookup::authoritative_zone(addr, fqdn)
            .await?
            .ok_or_else(|| {
                Refusal::NotWritable(format!(
                    "{addr} is not the primary of a zone that holds {}",
                    shown(fqdn)
                ))
            })?;
        let mut rs = RecordSet::with_ttl(fqdn.clone(), RecordType::TXT, TXT_TTL);
        rs.new_record(&RData::TXT(TXT::new(vec![value.to_string()])));
        let mut msg = if add {
            update_message::append(rs, zone.clone(), false, false)
        } else {
            update_message::delete_by_rdata(rs, zone.clone(), false)
        };
        let signer = self.signer()?;
        let verifier = msg
            .finalize(&signer, now_secs())
            .map_err(|e| format!("cannot sign the update: {e}"))?
            .ok_or_else(|| "cannot sign the update".to_string())?;
        let bytes = msg
            .to_vec()
            .map_err(|e| format!("cannot write the update: {e}"))?;
        let raw = wire::tcp(addr, &bytes, UPDATE_TIMEOUT).await?;
        let reply = wire::reply_to(&msg, &raw).map_err(|e| format!("{addr} sent {e}"))?;
        let ask = Asked {
            server: addr,
            key: &self.key_name,
            zone: &zone,
            fqdn,
        };
        ask.judge(&reply, &raw, verifier).map_err(Refusal::Failed)
    }
}

struct Asked<'a> {
    server: SocketAddr,
    key: &'a Name,
    zone: &'a Name,
    fqdn: &'a Name,
}

impl Asked<'_> {
    fn judge(&self, reply: &Message, raw: &[u8], mut verifier: TSigVerifier) -> Result<(), String> {
        let server = self.server;
        let key = shown(self.key);
        let code = reply.metadata.response_code;
        let signed = reply.signature().is_some_and(|s| !s.data.mac.is_empty());
        if !signed {
            let error = reply.signature().and_then(|s| s.data.error);
            return Err(match (code, error) {
                (ResponseCode::NotAuth, Some(TsigError::BadKey)) => format!(
                    "{server} does not know the key {key} (BADKEY): check key_name and that the key is hmac-sha256"
                ),
                (ResponseCode::NotAuth, Some(TsigError::BadSig)) => format!(
                    "{server} refused the signature of the key {key} (BADSIG): check the secret"
                ),
                (ResponseCode::NotAuth, Some(TsigError::BadTime)) => format!(
                    "{server} refused the update (BADTIME): its clock and this server's differ by more than {FUDGE} s"
                ),
                (ResponseCode::NotAuth, _) => self.not_auth(),
                _ => format!(
                    "{server} answered the signed update without a signature, so the answer was not trusted"
                ),
            });
        }
        verifier.verify(raw).map_err(|_| {
            format!("{server} answered with a TSIG signature that does not match the key {key}, so the answer was not trusted")
        })?;
        match code {
            ResponseCode::NoError => Ok(()),
            ResponseCode::NotAuth => Err(self.not_auth()),
            ResponseCode::NotZone => Err(format!(
                "{server} refused the update: {} is not in the zone {} (NOTZONE)",
                shown(self.fqdn),
                shown(self.zone)
            )),
            ResponseCode::Refused => Err(format!(
                "{server} refused the update of {}: check the zone's update policy for the key {key} (REFUSED)",
                shown(self.fqdn)
            )),
            other => Err(format!("{server} refused the update: {other}")),
        }
    }

    fn not_auth(&self) -> String {
        format!(
            "{} refused the update: Not authorized (check the key and the update policy of the zone {})",
            self.server,
            shown(self.zone)
        )
    }
}

impl Provider for Rfc2136 {
    fn present<'a>(&'a self, fqdn: &'a Name, value: &'a str) -> Fut<'a> {
        Box::pin(self.update(fqdn, value, true))
    }

    fn cleanup<'a>(&'a self, fqdn: &'a Name, value: &'a str) -> Fut<'a> {
        Box::pin(self.update(fqdn, value, false))
    }
}

#[cfg(test)]
mod tests;
