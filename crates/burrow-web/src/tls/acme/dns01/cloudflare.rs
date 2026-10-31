use std::collections::HashMap;
use std::sync::Mutex;

use hickory_proto::rr::Name;
use http::header::{AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER, USER_AGENT};
use http::{HeaderValue, Method, Request, StatusCode};
use serde_json::{Value, json};

use super::wire::shown;
use super::{Fut, Provider, Refusal, Secret, TXT_TTL};
use crate::tls::acme::http::Client;

pub const API: &str = "https://api.cloudflare.com/client/v4";
pub const IDENTICAL_RECORD: i64 = 81058;

pub struct Cloudflare {
    token: Secret,
    api: String,
    client: Client,
    zones: Mutex<HashMap<String, String>>,
    ids: Mutex<HashMap<(String, String), (String, String)>>,
}

struct Answer {
    status: StatusCode,
    errors: Vec<(i64, String)>,
    result: Value,
}

impl Answer {
    fn ok(&self) -> bool {
        self.status.is_success() && self.errors.is_empty()
    }

    fn text(&self, what: &str) -> String {
        let said: Vec<String> = self.errors.iter().map(|(_, msg)| msg.clone()).collect();
        format!(
            "Cloudflare {what}: HTTP {} {}",
            self.status.as_u16(),
            said.join("; ")
        )
        .trim_end()
        .to_string()
    }
}

fn quoted(value: &str) -> String {
    format!("\"{value}\"")
}

impl Cloudflare {
    pub fn new(token: Secret, api: Option<&str>, client: Client) -> Cloudflare {
        Cloudflare {
            token,
            api: api.unwrap_or(API).trim_end_matches('/').to_string(),
            client,
            zones: Mutex::new(HashMap::new()),
            ids: Mutex::new(HashMap::new()),
        }
    }

    fn bearer(&self) -> Result<HeaderValue, String> {
        let mut auth =
            HeaderValue::from_str(&format!("Bearer {}", self.token.expose())).map_err(|_| {
                "the Cloudflare token holds characters a header cannot carry".to_string()
            })?;
        auth.set_sensitive(true);
        Ok(auth)
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Answer, String> {
        let auth = self.bearer()?;
        let req = Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.api))
            .header(AUTHORIZATION, auth)
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, concat!("burrow/", env!("CARGO_PKG_VERSION")))
            .body(body.map_or_else(Vec::new, |b| b.to_string().into_bytes()))
            .map_err(|e| format!("cannot build the Cloudflare request: {e}"))?;
        let resp = self
            .client
            .fetch(req)
            .await
            .map_err(|e| format!("Cloudflare API: {e}"))?;
        let status = resp.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let wait = resp
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("a few minutes");
            return Err(format!(
                "Cloudflare API: HTTP 429, too many requests (its limit is 1,200 per 5 minutes per user); it asks to wait {wait} s"
            ));
        }
        let v: Value = serde_json::from_slice(resp.body()).unwrap_or(Value::Null);
        let errors = v["errors"]
            .as_array()
            .map(|list| {
                list.iter()
                    .map(|e| {
                        let code = e["code"].as_i64().unwrap_or(0);
                        (code, e["message"].as_str().unwrap_or("").to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Answer {
            status,
            errors,
            result: v["result"].clone(),
        })
    }

    async fn zone(&self, fqdn: &Name) -> Result<String, Refusal> {
        let full = shown(fqdn);
        let mut cand = full.as_str();
        let mut inactive = None;
        while let Some((_, parent)) = cand.split_once('.') {
            if !parent.contains('.') {
                break;
            }
            cand = parent;
            if let Some(id) = self.zones.lock().expect("zone lock").get(cand) {
                return Ok(id.clone());
            }
            let a = self
                .call(Method::GET, &format!("/zones?name={cand}"), None)
                .await?;
            if !a.ok() {
                return Err(Refusal::Failed(a.text("zone lookup")));
            }
            let found = a.result.as_array().cloned().unwrap_or_default();
            if let Some(z) = found.iter().find(|z| z["status"] == "active")
                && let Some(id) = z["id"].as_str()
            {
                self.zones
                    .lock()
                    .expect("zone lock")
                    .insert(cand.to_string(), id.to_string());
                return Ok(id.to_string());
            }
            if let Some(z) = found.first() {
                inactive = Some(format!(
                    "the Cloudflare zone {cand} is {}, not active, so its records are not public yet",
                    z["status"].as_str().unwrap_or("not active")
                ));
            }
        }
        Err(match inactive {
            Some(text) => Refusal::Failed(text),
            None => Refusal::NotWritable(format!(
                "no Cloudflare zone this token can see holds {full}; give the token Zone:Read and DNS:Edit for that zone"
            )),
        })
    }

    async fn find(&self, zone: &str, fqdn: &Name, value: &str) -> Result<Option<String>, Refusal> {
        let name = shown(fqdn);
        for content in [format!("%22{value}%22"), value.to_string()] {
            let path = format!(
                "/zones/{zone}/dns_records?type=TXT&name.exact={name}&content.exact={content}"
            );
            let a = self.call(Method::GET, &path, None).await?;
            if !a.ok() {
                return Err(Refusal::Failed(a.text("record lookup")));
            }
            if let Some(id) = a.result[0]["id"].as_str() {
                return Ok(Some(id.to_string()));
            }
        }
        Ok(None)
    }

    fn remember(&self, fqdn: &Name, value: &str, zone: &str, id: &str) {
        self.ids.lock().expect("id lock").insert(
            (shown(fqdn), value.to_string()),
            (zone.to_string(), id.to_string()),
        );
    }

    async fn add(&self, fqdn: &Name, value: &str) -> Result<(), Refusal> {
        let zone = self.zone(fqdn).await?;
        let body =
            json!({"type": "TXT", "name": shown(fqdn), "content": quoted(value), "ttl": TXT_TTL});
        let path = format!("/zones/{zone}/dns_records");
        let a = self.call(Method::POST, &path, Some(body)).await?;
        if a.ok() {
            if let Some(id) = a.result["id"].as_str() {
                self.remember(fqdn, value, &zone, id);
            }
            return Ok(());
        }
        if a.errors.iter().any(|(c, _)| *c == IDENTICAL_RECORD) {
            if let Some(id) = self.find(&zone, fqdn, value).await? {
                self.remember(fqdn, value, &zone, &id);
            }
            return Ok(());
        }
        Err(Refusal::Failed(a.text("add TXT")))
    }

    async fn remove(&self, fqdn: &Name, value: &str) -> Result<(), Refusal> {
        let known = self
            .ids
            .lock()
            .expect("id lock")
            .remove(&(shown(fqdn), value.to_string()));
        let (zone, id) = match known {
            Some(k) => k,
            None => {
                let zone = self.zone(fqdn).await?;
                match self.find(&zone, fqdn, value).await? {
                    Some(id) => (zone, id),
                    None => return Ok(()),
                }
            }
        };
        let a = self
            .call(
                Method::DELETE,
                &format!("/zones/{zone}/dns_records/{id}"),
                None,
            )
            .await?;
        if a.ok() || a.status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(Refusal::Failed(a.text("remove TXT")))
    }
}

impl Provider for Cloudflare {
    fn present<'a>(&'a self, fqdn: &'a Name, value: &'a str) -> Fut<'a> {
        Box::pin(self.add(fqdn, value))
    }

    fn cleanup<'a>(&'a self, fqdn: &'a Name, value: &'a str) -> Fut<'a> {
        Box::pin(self.remove(fqdn, value))
    }
}

#[cfg(test)]
mod tests;
