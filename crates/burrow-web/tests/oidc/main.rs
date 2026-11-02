#[path = "../common/mod.rs"]
mod common;
mod idp;

use std::collections::HashMap;

use burrow_web::{OidcProvider, Secret, SiteLogin};
use common::*;
use idp::{CLIENT, Grant, Idp, SECRET, idp};

const HOST: &str = "wiki.example.com";
const PAGE: &str = "Accept: text/html,application/xhtml+xml\r\n";

struct World {
    proxy: burrow_web::Proxy<Fake>,
    idp: Idp,
    _state: tempfile::TempDir,
}

struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Answer {
    fn parse(text: &str) -> Answer {
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text, ""));
        let mut lines = head.lines();
        let status = lines
            .next()
            .and_then(|l| l.split(' ').nth(1))
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let headers = lines
            .filter_map(|l| l.split_once(": "))
            .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
            .collect();
        Answer {
            status,
            headers,
            body: body.to_string(),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn cookies(&self) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k == "set-cookie")
            .map(|(_, v)| v.as_str())
            .collect()
    }

    fn cookie_pair(&self, prefix: &str) -> String {
        let c = self.cookies().into_iter().find(|c| c.starts_with(prefix));
        c.unwrap_or_else(|| panic!("no {prefix} cookie: {:?}", self.headers))
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }
}

fn provider(name: &str, idp: &Idp) -> OidcProvider {
    OidcProvider {
        name: name.into(),
        issuer: idp.issuer(),
        client_id: CLIENT.into(),
        client_secret: Some(Secret::new(SECRET.into())),
    }
}

async fn world(providers: &[&str], allow: &[&str]) -> World {
    let app = http_backend().await;
    let idp = idp().await;
    let (up, _) = Fake::new(4);
    up.serve("wiki", app.addr);
    let state = tempfile::tempdir().unwrap();
    let mut site = https_site("wiki", &[HOST]);
    site.options.oidc = Some(SiteLogin {
        providers: providers.iter().map(|p| p.to_string()).collect(),
        allow_users: allow.iter().map(|a| a.to_string()).collect(),
    });
    let list: Vec<OidcProvider> = providers.iter().map(|p| provider(p, &idp)).collect();
    let dir = state.path().to_path_buf();
    let proxy = start(
        move |s| {
            s.sites = vec![site, https_site("open", &["open.example.com"])];
            s.oidc = list;
            s.certs.state_dir = Some(dir);
        },
        up,
    )
    .await;
    World {
        proxy,
        idp,
        _state: state,
    }
}

impl World {
    async fn get(&self, target: &str, extra: &str) -> Answer {
        let req =
            format!("GET {target} HTTP/1.1\r\nHost: {HOST}\r\n{extra}Connection: close\r\n\r\n");
        Answer::parse(&over_tls(self.proxy.https_addr().unwrap(), HOST, &req).await)
    }

    async fn log_in(&self, email: &str, forged: bool) -> Answer {
        let first = self.get("/page?x=1", PAGE).await;
        assert_eq!(first.status, 303, "{}", first.body);
        self.finish(&first, email, forged).await
    }

    async fn finish(&self, started: &Answer, email: &str, forged: bool) -> Answer {
        let to = started.header("location").unwrap();
        let (base, q) = to.split_once('?').unwrap();
        assert_eq!(base, format!("{}/authorize", self.idp.issuer()));
        let q: HashMap<String, String> = url::form_urlencoded::parse(q.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["client_id"], CLIENT);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["scope"], "openid email");
        assert_eq!(
            q["redirect_uri"],
            format!("https://{HOST}/.burrow/login/callback")
        );
        self.idp.grant(
            "c1",
            Grant {
                email: email.into(),
                nonce: q["nonce"].clone(),
                challenge: q["code_challenge"].clone(),
                redirect: q["redirect_uri"].clone(),
                forged,
            },
        );
        let pending = started.cookie_pair("__Host-burrow-login-");
        let target = format!("/.burrow/login/callback?code=c1&state={}", q["state"]);
        self.get(&target, &format!("Cookie: {pending}\r\n")).await
    }
}

#[tokio::test]
async fn an_allowed_user_logs_in_with_pkce_and_the_app_gets_their_email_only() {
    let w = world(&["idp"], &["alice@example.com"]).await;
    let back = w.log_in("alice@example.com", false).await;
    assert_eq!(back.status, 303, "{}", back.body);
    assert_eq!(back.header("location"), Some("/page?x=1"));
    let session = back.cookie_pair("__Host-burrow-session=");
    let set = back
        .cookies()
        .into_iter()
        .find(|c| c.starts_with("__Host-burrow-session="))
        .unwrap();
    assert!(
        set.ends_with("; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=43200"),
        "{set}"
    );
    assert!(
        back.cookies()
            .iter()
            .any(|c| c.starts_with("__Host-burrow-login-") && c.ends_with("Max-Age=0"))
    );
    {
        let seen = w.idp.seen.lock().unwrap();
        assert_eq!(seen.token_calls, 1);
        let basic = seen.auth_header.as_deref().unwrap_or_default();
        assert!(basic.starts_with("Basic "), "client_secret_basic: {basic}");
    }
    let extra =
        format!("Cookie: theme=dark; {session}\r\nRemote-User: root\r\nRemote_Email: root@x\r\n");
    let app = w.get("/page?x=1", &extra).await;
    assert_eq!(app.status, 200, "{}", app.body);
    let seen = app.body.to_ascii_lowercase();
    assert!(seen.contains("remote-user: alice@example.com\n"), "{seen}");
    assert!(seen.contains("remote-email: alice@example.com\n"), "{seen}");
    assert!(!seen.contains("root"), "{seen}");
    assert!(seen.contains("cookie: theme=dark\n"), "{seen}");
    assert!(!seen.contains("__host-burrow"), "{seen}");
    let api = w
        .get(
            "/api",
            &format!("Cookie: {session}\r\nAccept: application/json\r\n"),
        )
        .await;
    assert_eq!(api.status, 200, "a session works for scripts too");
}

#[tokio::test]
async fn scripts_and_tools_without_a_session_get_401_instead_of_a_redirect() {
    let w = world(&["idp"], &["alice@example.com"]).await;
    assert_eq!(w.get("/", "Accept: application/json\r\n").await.status, 401);
    assert_eq!(w.get("/", "").await.status, 401);
    let xhr = format!("{PAGE}X-Requested-With: XMLHttpRequest\r\n");
    assert_eq!(w.get("/", &xhr).await.status, 401);
    let fetch = format!("{PAGE}Sec-Fetch-Mode: cors\r\n");
    assert_eq!(w.get("/", &fetch).await.status, 401);
    assert_eq!(
        w.idp.seen.lock().unwrap().discovered,
        0,
        "no provider call for them"
    );
}

#[tokio::test]
async fn a_user_outside_allow_users_gets_403_and_no_session() {
    let w = world(&["idp"], &["alice@example.com", "@team.example"]).await;
    let back = w.log_in("mallory@other.example", false).await;
    assert_eq!(back.status, 403, "{}", back.body);
    assert!(
        back.body
            .contains("mallory@other.example may not use this site"),
        "{}",
        back.body
    );
    assert!(
        back.cookies()
            .iter()
            .all(|c| !c.starts_with("__Host-burrow-session="))
    );
    let team = w.log_in("bob@team.example", false).await;
    assert_eq!(team.status, 303, "a whole domain is allowed");
}

#[tokio::test]
async fn a_token_signed_with_another_key_or_a_stale_callback_logs_nobody_in() {
    let w = world(&["idp"], &["alice@example.com"]).await;
    let forged = w.log_in("alice@example.com", true).await;
    assert_eq!(forged.status, 502, "{}", forged.body);
    assert!(
        forged
            .cookies()
            .iter()
            .all(|c| !c.starts_with("__Host-burrow-session="))
    );
    let stale = w
        .get("/.burrow/login/callback?code=c1&state=nope", "")
        .await;
    assert_eq!(stale.status, 400, "{}", stale.body);
    let first = w.get("/page", PAGE).await;
    let other = w.get("/page", PAGE).await;
    let q = first
        .header("location")
        .unwrap()
        .split_once('?')
        .unwrap()
        .1
        .to_string();
    let state: HashMap<String, String> = url::form_urlencoded::parse(q.as_bytes())
        .into_owned()
        .collect();
    let wrong = other.cookie_pair("__Host-burrow-login-");
    let target = format!("/.burrow/login/callback?code=c1&state={}", state["state"]);
    let mixed = w.get(&target, &format!("Cookie: {wrong}\r\n")).await;
    assert_eq!(
        mixed.status, 400,
        "another login's cookie does not fit this state"
    );
    let cancelled = format!(
        "/.burrow/login/callback?error=access_denied&state={}",
        state["state"]
    );
    let mine = first.cookie_pair("__Host-burrow-login-");
    let said = w.get(&cancelled, &format!("Cookie: {mine}\r\n")).await;
    assert_eq!(said.status, 403);
    assert!(said.body.contains("access_denied"), "{}", said.body);
}

#[tokio::test]
async fn several_providers_offer_a_chooser_whose_links_start_the_login() {
    let w = world(&["idp", "work"], &["alice@example.com"]).await;
    let page = w.get("/docs?a=1", PAGE).await;
    assert_eq!(page.status, 200, "{}", page.body);
    assert!(
        page.body
            .contains("href=\"/.burrow/login?with=idp&amp;to=%2Fdocs%3Fa%3D1\""),
        "{}",
        page.body
    );
    assert!(page.body.contains("Log in with work"), "{}", page.body);
    let started = w
        .get("/.burrow/login?with=work&to=%2Fdocs%3Fa%3D1", "")
        .await;
    assert_eq!(started.status, 303, "{}", started.body);
    let back = w.finish(&started, "alice@example.com", false).await;
    assert_eq!(back.header("location"), Some("/docs?a=1"));
    let unknown = w.get("/.burrow/login?with=nope&to=/", "").await;
    assert_eq!(unknown.status, 404);
    let away = w
        .get("/.burrow/login?with=work&to=%2F%2Fevil.example.com", "")
        .await;
    let q = away
        .header("location")
        .unwrap()
        .split_once('?')
        .unwrap()
        .1
        .to_string();
    let q: HashMap<String, String> = url::form_urlencoded::parse(q.as_bytes())
        .into_owned()
        .collect();
    w.idp.grant(
        "c2",
        Grant {
            email: "alice@example.com".into(),
            nonce: q["nonce"].clone(),
            challenge: q["code_challenge"].clone(),
            redirect: q["redirect_uri"].clone(),
            forged: false,
        },
    );
    let pending = away.cookie_pair("__Host-burrow-login-");
    let back = w
        .get(
            &format!("/.burrow/login/callback?code=c2&state={}", q["state"]),
            &format!("Cookie: {pending}\r\n"),
        )
        .await;
    assert_eq!(
        back.header("location"),
        Some("/"),
        "never back to another site"
    );
}

#[tokio::test]
async fn logout_clears_the_session_and_a_site_without_oidc_keeps_its_paths() {
    let w = world(&["idp"], &["alice@example.com"]).await;
    let back = w.log_in("alice@example.com", false).await;
    let session = back.cookie_pair("__Host-burrow-session=");
    let out = w
        .get("/.burrow/logout", &format!("Cookie: {session}\r\n"))
        .await;
    assert_eq!(out.status, 200);
    assert!(
        out.cookies()
            .contains(&"__Host-burrow-session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0")
    );
    let req = "GET /.burrow/logout HTTP/1.1\r\nHost: open.example.com\r\nConnection: close\r\n\r\n";
    let open =
        Answer::parse(&over_tls(w.proxy.https_addr().unwrap(), "open.example.com", req).await);
    assert_eq!(
        open.status, 503,
        "the path belongs to the app there, which is not running: {}",
        open.body
    );
}

#[tokio::test]
async fn a_site_with_oidc_is_never_served_on_the_plain_port() {
    let idp = idp().await;
    let (up, _) = Fake::new(4);
    let mut site = https_site("wiki", &[HOST]);
    site.https_redirect = false;
    site.options.oidc = Some(SiteLogin {
        providers: vec!["idp".into()],
        allow_users: vec!["alice@example.com".into()],
    });
    let list = vec![provider("idp", &idp)];
    let proxy = start_with(
        move |s| {
            s.sites = vec![site];
            s.oidc = list;
        },
        up,
        fixed(&[HOST]),
        true,
    )
    .await;
    let req = format!("GET /a HTTP/1.1\r\nHost: {HOST}\r\n{PAGE}Connection: close\r\n\r\n");
    let r = Answer::parse(&over_plain(proxy.http_addr().unwrap(), &req).await);
    assert_eq!(r.status, 308, "{}", r.body);
    let req = format!("GET /.burrow/logout HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n\r\n");
    let r = Answer::parse(&over_plain(proxy.http_addr().unwrap(), &req).await);
    assert_eq!(r.status, 308, "{}", r.body);
}
