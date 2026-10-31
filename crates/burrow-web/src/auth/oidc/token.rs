use std::future::Future;
use std::pin::Pin;

use oauth2::basic::{
    BasicErrorResponse, BasicRevocationErrorResponse, BasicTokenIntrospectionResponse,
    BasicTokenType,
};
use oauth2::{
    AsyncHttpClient, AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    EndpointNotSet, EndpointSet, ExtraTokenFields, HttpRequest, HttpResponse, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, RequestTokenError, Scope, StandardRevocableToken,
    StandardTokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};

use super::provider::{Known, Provider};
use crate::tls::acme::http::Client;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct IdTokenField {
    id_token: String,
}

impl ExtraTokenFields for IdTokenField {}

type Answer = StandardTokenResponse<IdTokenField, BasicTokenType>;

type OAuthClient = oauth2::Client<
    BasicErrorResponse,
    Answer,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
>;

pub(crate) struct Started {
    pub url: String,
    pub state: String,
    pub verifier: String,
    pub nonce: String,
}

#[derive(Debug)]
struct Failed(String);

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failed {}

struct Http(Client);

type Call<'c> = Pin<Box<dyn Future<Output = Result<HttpResponse, Failed>> + Send + 'c>>;

impl<'c> AsyncHttpClient<'c> for Http {
    type Error = Failed;
    type Future = Call<'c>;

    fn call(&'c self, req: HttpRequest) -> Call<'c> {
        Box::pin(async move {
            let resp = self.0.fetch(req).await.map_err(Failed)?;
            Ok(resp.map(|b| b.to_vec()))
        })
    }
}

fn oauth(p: &Provider, k: &Known, redirect: &str) -> Result<OAuthClient, String> {
    let bad = |what: &str, e: url::ParseError| format!("{what}: {e}");
    let mut c = oauth2::Client::new(ClientId::new(p.cfg.client_id.clone()))
        .set_auth_uri(
            AuthUrl::new(k.meta.authorization_endpoint.clone())
                .map_err(|e| bad("authorization_endpoint", e))?,
        )
        .set_token_uri(
            TokenUrl::new(k.meta.token_endpoint.clone()).map_err(|e| bad("token_endpoint", e))?,
        )
        .set_redirect_uri(RedirectUrl::new(redirect.to_string()).map_err(|e| bad("redirect", e))?);
    if let Some(s) = &p.cfg.client_secret {
        c = c.set_client_secret(ClientSecret::new(s.expose().to_string()));
    }
    let methods = &k.meta.token_endpoint_auth_methods_supported;
    let post_only = !methods.is_empty()
        && !methods.iter().any(|m| m == "client_secret_basic")
        && methods.iter().any(|m| m == "client_secret_post");
    if post_only || p.cfg.client_secret.is_none() {
        c = c.set_auth_type(AuthType::RequestBody);
    }
    Ok(c)
}

pub(crate) async fn start(p: &Provider, redirect: &str) -> Result<Started, String> {
    let k = p.known(false).await?;
    let c = oauth(p, &k, redirect)?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let nonce = CsrfToken::new_random();
    let (url, state) = c
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("openid".into()))
        .add_scope(Scope::new("email".into()))
        .add_extra_param("nonce", nonce.secret())
        .set_pkce_challenge(challenge)
        .url();
    Ok(Started {
        url: url.to_string(),
        state: state.secret().clone(),
        verifier: verifier.secret().clone(),
        nonce: nonce.secret().clone(),
    })
}

pub(crate) async fn exchange(
    p: &Provider,
    redirect: &str,
    code: &str,
    verifier: &str,
) -> Result<String, String> {
    let k = p.known(false).await?;
    let c = oauth(p, &k, redirect)?;
    let http = Http(p.client().clone());
    let answer = c
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .set_pkce_verifier(PkceCodeVerifier::new(verifier.to_string()))
        .request_async(&http)
        .await
        .map_err(|e| match e {
            RequestTokenError::ServerResponse(r) => format!("the provider refused the code: {r}"),
            RequestTokenError::Request(f) => format!("the token request failed: {f}"),
            RequestTokenError::Parse(e, _) => format!("the token answer: {e}"),
            RequestTokenError::Other(o) => o,
        })?;
    Ok(answer.extra_fields().id_token.clone())
}
