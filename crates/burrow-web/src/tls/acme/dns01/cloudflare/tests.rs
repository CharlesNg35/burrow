use std::sync::{Arc, Mutex};

use http::{Method, StatusCode};
use serde_json::{Value, json};

use super::super::mock_api::{Api, Seen, envelope, failure};
use super::super::wire::name;
use super::*;

#[derive(Default)]
struct Zone {
    records: Vec<(String, String, String)>,
    next: u32,
    limited: bool,
}

fn query(path: &str, key: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    q.split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .map(|v| v.replace("%22", "\""))
}

fn handle(z: &Mutex<Zone>, s: &Seen) -> (StatusCode, Value) {
    let mut z = z.lock().unwrap();
    if z.limited {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            971,
            "Please wait and consider throttling your request speed",
        );
    }
    if s.headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer good") {
        return failure(StatusCode::FORBIDDEN, 10000, "Authentication error");
    }
    let path = s.path.as_str();
    if path.starts_with("/zones?") {
        let found = match query(path, "name").as_deref() {
            Some("example.com") => json!([{"id": "z1", "name": "example.com", "status": "active"}]),
            Some("pending.example.org") => {
                json!([{"id": "z2", "name": "pending.example.org", "status": "pending"}])
            }
            _ => json!([]),
        };
        return envelope(found);
    }
    if s.method == Method::POST && path == "/zones/z1/dns_records" {
        let (n, c) = (
            s.body["name"].as_str().unwrap().to_string(),
            s.body["content"].as_str().unwrap().to_string(),
        );
        if z.records.iter().any(|(_, rn, rc)| *rn == n && *rc == c) {
            return failure(
                StatusCode::BAD_REQUEST,
                81058,
                "An identical record already exists.",
            );
        }
        z.next += 1;
        let id = format!("r{}", z.next);
        z.records.push((id.clone(), n, c));
        return envelope(json!({"id": id}));
    }
    if s.method == Method::GET && path.starts_with("/zones/z1/dns_records?") {
        let (n, c) = (query(path, "name.exact"), query(path, "content.exact"));
        let hits: Vec<Value> = z
            .records
            .iter()
            .filter(|(_, rn, rc)| Some(rn) == n.as_ref() && Some(rc) == c.as_ref())
            .map(|(id, _, _)| json!({"id": id}))
            .collect();
        return envelope(Value::Array(hits));
    }
    if let Some(id) = path.strip_prefix("/zones/z1/dns_records/") {
        let before = z.records.len();
        z.records.retain(|(rid, _, _)| rid != id);
        if z.records.len() == before {
            return failure(StatusCode::NOT_FOUND, 81044, "Record does not exist.");
        }
        return envelope(json!({"id": id}));
    }
    failure(StatusCode::BAD_REQUEST, 7003, "Could not route")
}

async fn api() -> (Api, Arc<Mutex<Zone>>) {
    let zone = Arc::new(Mutex::new(Zone::default()));
    let z = zone.clone();
    (
        Api::start(Arc::new(move |s: &Seen| handle(&z, s))).await,
        zone,
    )
}

fn cloudflare(a: &Api, token: &str) -> Cloudflare {
    Cloudflare::new(Secret::new(token.into()), Some(&a.url), a.client())
}

const FQDN: &str = "_acme-challenge.example.com";

#[tokio::test]
async fn a_wildcard_and_its_bare_name_get_two_quoted_values_side_by_side() {
    let (a, zone) = api().await;
    let cf = cloudflare(&a, "good");
    let n = name(FQDN).unwrap();
    cf.present(&n, "wild").await.unwrap();
    cf.present(&n, "bare").await.unwrap();
    let records: Vec<(String, String)> = zone
        .lock()
        .unwrap()
        .records
        .iter()
        .map(|(_, n, c)| (n.clone(), c.clone()))
        .collect();
    assert_eq!(
        records,
        vec![
            (FQDN.to_string(), "\"wild\"".to_string()),
            (FQDN.to_string(), "\"bare\"".to_string())
        ]
    );
    let post = a
        .seen()
        .into_iter()
        .find(|s| s.method == Method::POST)
        .unwrap();
    assert_eq!(post.body["ttl"], 60);
    assert_eq!(post.body["type"], "TXT");
    cf.cleanup(&n, "wild").await.unwrap();
    cf.cleanup(&n, "bare").await.unwrap();
    assert!(zone.lock().unwrap().records.is_empty());
    assert!(cf.bearer().unwrap().is_sensitive());
}

#[tokio::test]
async fn a_wrong_token_or_the_rate_limit_is_shown_with_cloudflare_s_reason() {
    let (a, zone) = api().await;
    let err = cloudflare(&a, "bad")
        .present(&name(FQDN).unwrap(), "v")
        .await
        .unwrap_err();
    assert_eq!(
        err,
        Refusal::Failed("Cloudflare zone lookup: HTTP 403 Authentication error".into())
    );
    zone.lock().unwrap().limited = true;
    let err = cloudflare(&a, "good")
        .present(&name(FQDN).unwrap(), "v")
        .await
        .unwrap_err();
    assert!(
        err.text().ends_with("it asks to wait 300 s"),
        "{}",
        err.text()
    );
}

#[tokio::test]
async fn an_identical_record_counts_as_set_and_is_still_cleaned_up() {
    let (a, zone) = api().await;
    zone.lock()
        .unwrap()
        .records
        .push(("old".into(), FQDN.into(), "\"v\"".into()));
    let cf = cloudflare(&a, "good");
    let n = name(FQDN).unwrap();
    cf.present(&n, "v").await.unwrap();
    cf.cleanup(&n, "v").await.unwrap();
    assert!(zone.lock().unwrap().records.is_empty());
}

#[tokio::test]
async fn a_record_from_before_a_restart_is_found_and_removed() {
    let (a, zone) = api().await;
    let n = name(FQDN).unwrap();
    cloudflare(&a, "good").present(&n, "v").await.unwrap();
    zone.lock()
        .unwrap()
        .records
        .push(("bare".into(), FQDN.into(), "w".into()));
    let fresh = cloudflare(&a, "good");
    fresh.cleanup(&n, "v").await.unwrap();
    fresh.cleanup(&n, "w").await.unwrap();
    fresh.cleanup(&n, "never-set").await.unwrap();
    assert!(zone.lock().unwrap().records.is_empty());
}

#[tokio::test]
async fn a_name_in_no_visible_or_no_active_zone_is_refused() {
    let (a, _) = api().await;
    let cf = cloudflare(&a, "good");
    let err = cf
        .present(&name("erp.acme.example.net").unwrap(), "v")
        .await
        .unwrap_err();
    assert_eq!(
        err,
        Refusal::NotWritable(
            "no Cloudflare zone this token can see holds erp.acme.example.net; give the token Zone:Read and DNS:Edit for that zone".into()
        )
    );
    let err = cf
        .present(&name("_acme-challenge.a.pending.example.org").unwrap(), "v")
        .await
        .unwrap_err();
    assert_eq!(
        err,
        Refusal::Failed("the Cloudflare zone pending.example.org is pending, not active, so its records are not public yet".into())
    );
}
