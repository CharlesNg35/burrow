use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use tokio::net::TcpListener;

pub const CLIENT: &str = "burrow-test";
pub const SECRET: &str = "s3cret";

#[derive(Clone)]
pub struct Grant {
    pub email: String,
    pub nonce: String,
    pub challenge: String,
    pub redirect: String,
    pub forged: bool,
}

#[derive(Default)]
pub struct Seen {
    pub grants: HashMap<String, Grant>,
    pub token_calls: usize,
    pub auth_header: Option<String>,
    pub discovered: usize,
}

pub struct Idp {
    pub addr: SocketAddr,
    pub seen: Arc<Mutex<Seen>>,
}

struct Keys {
    good: EncodingKey,
    forged: EncodingKey,
    set: JwkSet,
}

fn key() -> EncodingKey {
    let kp = rcgen::KeyPair::generate().unwrap();
    EncodingKey::from_ec_der(&kp.serialize_der())
}

fn keys() -> Keys {
    let good = key();
    let mut jwk = Jwk::from_encoding_key(&good, Algorithm::ES256).unwrap();
    jwk.common.key_id = Some("k1".into());
    Keys {
        good,
        forged: key(),
        set: JwkSet { keys: vec![jwk] },
    }
}

impl Idp {
    pub fn issuer(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn grant(&self, code: &str, g: Grant) {
        self.seen.lock().unwrap().grants.insert(code.into(), g);
    }
}

fn json_answer(status: StatusCode, v: &Value) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from(v.to_string())));
    *r.status_mut() = status;
    r.headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    r
}

fn s256(verifier: &str) -> String {
    let d = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(d.as_ref())
}

fn id_token(k: &Keys, issuer: &str, g: &Grant) -> String {
    let now = burrow_syntax::duration::now_secs();
    let claims = json!({
        "iss": issuer, "aud": CLIENT, "sub": "123", "iat": now, "exp": now + 300,
        "nonce": g.nonce, "email": g.email, "email_verified": true
    });
    let mut h = Header::new(Algorithm::ES256);
    h.kid = Some("k1".into());
    let key = if g.forged { &k.forged } else { &k.good };
    encode(&h, &claims, key).unwrap()
}

async fn token(
    req: Request<Incoming>,
    k: &Keys,
    issuer: &str,
    seen: &Mutex<Seen>,
) -> Response<Full<Bytes>> {
    let auth = req
        .headers()
        .get("authorization")
        .map(|v| v.to_str().unwrap().to_string());
    let body = req.into_body().collect().await.unwrap().to_bytes();
    let form: HashMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
    let mut s = seen.lock().unwrap();
    s.token_calls += 1;
    s.auth_header = auth;
    let code = form.get("code").cloned().unwrap_or_default();
    let Some(g) = s.grants.remove(&code) else {
        return json_answer(StatusCode::BAD_REQUEST, &json!({"error": "invalid_grant"}));
    };
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let fits = s256(&verifier) == g.challenge
        && form.get("redirect_uri") == Some(&g.redirect)
        && form.get("grant_type").map(String::as_str) == Some("authorization_code");
    if !fits {
        return json_answer(StatusCode::BAD_REQUEST, &json!({"error": "invalid_grant"}));
    }
    let t = id_token(k, issuer, &g);
    json_answer(
        StatusCode::OK,
        &json!({"access_token": "at", "token_type": "Bearer", "expires_in": 300, "id_token": t}),
    )
}

async fn answer(
    req: Request<Incoming>,
    k: Arc<Keys>,
    issuer: String,
    seen: Arc<Mutex<Seen>>,
) -> Response<Full<Bytes>> {
    match req.uri().path() {
        "/.well-known/openid-configuration" => {
            seen.lock().unwrap().discovered += 1;
            json_answer(
                StatusCode::OK,
                &json!({
                    "issuer": issuer,
                    "authorization_endpoint": format!("{issuer}/authorize"),
                    "token_endpoint": format!("{issuer}/token"),
                    "jwks_uri": format!("{issuer}/jwks"),
                    "token_endpoint_auth_methods_supported": ["client_secret_basic"]
                }),
            )
        }
        "/jwks" => json_answer(StatusCode::OK, &serde_json::to_value(&k.set).unwrap()),
        "/token" => token(req, &k, &issuer, &seen).await,
        _ => json_answer(StatusCode::NOT_FOUND, &json!({})),
    }
}

pub async fn idp() -> Idp {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let k = Arc::new(keys());
    let issuer = format!("http://{addr}");
    let kept = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            let (k, issuer, seen) = (k.clone(), issuer.clone(), kept.clone());
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(move |req| {
                    let (k, issuer, seen) = (k.clone(), issuer.clone(), seen.clone());
                    async move { Ok::<_, std::convert::Infallible>(answer(req, k, issuer, seen).await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    Idp { addr, seen }
}
