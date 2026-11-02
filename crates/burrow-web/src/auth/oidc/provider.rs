use std::sync::Arc;
use std::time::{Duration, Instant};

use burrow_syntax::web::oidc::discovery_url;
use http::{Method, Request, header};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, AlgorithmFamily, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;

use crate::OidcProvider;
use crate::tls::acme::http::{Client, Limits};

pub const FRESH: Duration = Duration::from_secs(3600);
pub const REFETCH_GAP: Duration = Duration::from_secs(30);
pub const LEEWAY_SECS: u64 = 60;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Meta {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Vec<String>,
}

pub(crate) struct Known {
    pub meta: Meta,
    keys: JwkSet,
    at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Identity {
    pub email: String,
}

#[derive(Deserialize)]
struct Claims {
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<serde_json::Value>,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    aud: serde_json::Value,
}

pub(crate) struct Provider {
    pub cfg: OidcProvider,
    client: Client,
    known: Mutex<Option<Arc<Known>>>,
}

fn limits() -> Limits {
    Limits {
        connect: Duration::from_secs(10),
        request: Duration::from_secs(15),
        body: 256 * 1024,
    }
}

fn verified(v: Option<&serde_json::Value>) -> bool {
    match v {
        None => true,
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s.eq_ignore_ascii_case("true"),
        Some(_) => false,
    }
}

fn asymmetric(alg: Algorithm) -> bool {
    !matches!(alg.family(), AlgorithmFamily::Hmac)
}

impl Provider {
    pub(crate) fn new(cfg: OidcProvider) -> Provider {
        let client = Client::new(None, limits())
            .expect("the public roots")
            .with_plain_private();
        Provider {
            cfg,
            client,
            known: Mutex::new(None),
        }
    }

    pub(crate) fn client(&self) -> &Client {
        &self.client
    }

    async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, String> {
        let req = Request::builder()
            .method(Method::GET)
            .uri(url)
            .header(header::ACCEPT, "application/json")
            .body(Vec::new())
            .map_err(|e| format!("{url}: {e}"))?;
        let resp = self
            .client
            .fetch(req)
            .await
            .map_err(|e| format!("{url}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("{url} answered {}", resp.status()));
        }
        serde_json::from_slice(resp.body()).map_err(|e| format!("{url}: {e}"))
    }

    async fn fetch(&self) -> Result<Known, String> {
        let meta: Meta = self.get_json(&discovery_url(&self.cfg.issuer)).await?;
        if meta.issuer != self.cfg.issuer.trim() {
            return Err(format!(
                "the provider calls itself \"{}\", not \"{}\"; set issuer to exactly that",
                meta.issuer,
                self.cfg.issuer.trim()
            ));
        }
        let keys: JwkSet = self.get_json(&meta.jwks_uri).await?;
        Ok(Known {
            meta,
            keys,
            at: Instant::now(),
        })
    }

    pub(crate) async fn known(&self, renew_keys: bool) -> Result<Arc<Known>, String> {
        let mut held = self.known.lock().await;
        if let Some(k) = held.as_ref() {
            let age = k.at.elapsed();
            let fresh = age < FRESH && !renew_keys;
            if fresh || (renew_keys && age < REFETCH_GAP) {
                return Ok(k.clone());
            }
        }
        let k = Arc::new(self.fetch().await?);
        *held = Some(k.clone());
        Ok(k)
    }

    pub(crate) async fn verify(&self, id_token: &str, nonce: &str) -> Result<Identity, String> {
        let head = decode_header(id_token).map_err(|e| format!("the ID token: {e}"))?;
        if !asymmetric(head.alg) {
            return Err(format!(
                "the ID token is signed with {:?}, which burrow does not take",
                head.alg
            ));
        }
        let mut known = self.known(false).await?;
        let key = match key_for(&known.keys, head.kid.as_deref(), head.alg) {
            Some(k) => k,
            None => {
                known = self.known(true).await?;
                key_for(&known.keys, head.kid.as_deref(), head.alg)
                    .ok_or("the ID token is signed with a key the provider does not list")?
            }
        };
        let mut v = Validation::new(head.alg);
        v.set_issuer(&[&known.meta.issuer]);
        v.set_audience(&[&self.cfg.client_id]);
        v.set_required_spec_claims(&["exp", "iss", "aud"]);
        v.leeway = LEEWAY_SECS;
        let claims = decode::<Claims>(id_token, &key, &v)
            .map_err(|e| format!("the ID token: {e}"))?
            .claims;
        check_claims(&claims, nonce, &self.cfg.client_id)
    }
}

fn key_for(keys: &JwkSet, kid: Option<&str>, alg: Algorithm) -> Option<DecodingKey> {
    let fits = |k: &&jsonwebtoken::jwk::Jwk| {
        k.common
            .key_algorithm
            .is_none_or(|a| a == jsonwebtoken::jwk::KeyAlgorithm::from(alg))
    };
    let jwk = match kid {
        Some(kid) => keys.find(kid).filter(fits),
        None => keys
            .keys
            .iter()
            .filter(fits)
            .find(|k| DecodingKey::from_jwk(k).is_ok()),
    }?;
    DecodingKey::from_jwk(jwk).ok()
}

fn check_claims(c: &Claims, nonce: &str, client_id: &str) -> Result<Identity, String> {
    if c.nonce.as_deref() != Some(nonce) {
        return Err("the ID token is for another login (nonce)".into());
    }
    let many = c.aud.as_array().is_some_and(|a| a.len() > 1);
    if many && c.azp.as_deref().is_some_and(|azp| azp != client_id) {
        return Err("the ID token was issued to another client (azp)".into());
    }
    let email = c
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .ok_or("the provider gave no email address; ask it for the email scope")?;
    if !verified(c.email_verified.as_ref()) {
        return Err(format!("the provider has not verified {email}"));
    }
    Ok(Identity {
        email: email.to_string(),
    })
}

#[cfg(test)]
mod tests;
