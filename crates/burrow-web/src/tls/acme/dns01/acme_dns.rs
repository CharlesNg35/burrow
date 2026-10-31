use hickory_proto::rr::Name;
use http::header::{CONTENT_TYPE, USER_AGENT};
use http::{HeaderValue, Method, Request};
use serde_json::json;

use super::wire::shown;
use super::{Fut, Provider, Refusal, Secret};
use crate::tls::acme::http::Client;

pub struct AcmeDns {
    api: String,
    username: String,
    password: Secret,
    subdomain: String,
    client: Client,
}

impl AcmeDns {
    pub fn new(
        api: &str,
        username: &str,
        password: Secret,
        subdomain: &str,
        client: Client,
    ) -> AcmeDns {
        AcmeDns {
            api: api.trim_end_matches('/').to_string(),
            username: username.to_string(),
            password,
            subdomain: subdomain.to_ascii_lowercase(),
            client,
        }
    }

    async fn update(&self, fqdn: &Name, value: &str) -> Result<(), Refusal> {
        let target = shown(fqdn);
        if target.split('.').next() != Some(self.subdomain.as_str()) {
            return Err(Refusal::Failed(format!(
                "the record would go to {target}, but acme-dns answers for {sub}.<its domain>: make _acme-challenge of this name a CNAME to {sub}.<your acme-dns domain>",
                sub = self.subdomain
            )));
        }
        let mut key = HeaderValue::from_str(self.password.expose()).map_err(|_| {
            "the acme-dns password holds characters a header cannot carry".to_string()
        })?;
        key.set_sensitive(true);
        let user = HeaderValue::from_str(&self.username).map_err(|_| {
            "the acme-dns username holds characters a header cannot carry".to_string()
        })?;
        let body = json!({"subdomain": self.subdomain, "txt": value}).to_string();
        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("{}/update", self.api))
            .header("x-api-user", user)
            .header("x-api-key", key)
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, concat!("burrow/", env!("CARGO_PKG_VERSION")))
            .body(body.into_bytes())
            .map_err(|e| format!("cannot build the acme-dns request: {e}"))?;
        let resp = self
            .client
            .fetch(req)
            .await
            .map_err(|e| format!("acme-dns update: {e}"))?;
        if resp.status().is_success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(resp.body());
        Err(Refusal::Failed(
            format!(
                "acme-dns update: HTTP {} {}",
                resp.status().as_u16(),
                super::command::tail(&said)
            )
            .trim_end()
            .to_string(),
        ))
    }
}

impl Provider for AcmeDns {
    fn present<'a>(&'a self, fqdn: &'a Name, value: &'a str) -> Fut<'a> {
        Box::pin(self.update(fqdn, value))
    }

    fn cleanup<'a>(&'a self, _fqdn: &'a Name, _value: &'a str) -> Fut<'a> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests;
