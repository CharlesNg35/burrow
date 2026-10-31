mod common;

use std::num::NonZeroU32;

use burrow_web::auth::basic::encode;
use burrow_web::{BasicUser, ProxySettings, Site};
use common::*;

fn trusted() -> Vec<burrow_syntax::cidr::Cidr> {
    vec!["127.0.0.1/32".parse().unwrap()]
}

fn protected(hash: &str) -> Site {
    let mut s = https_site("grafana", &["grafana.example.com"]);
    s.options.basic_auth = vec![BasicUser {
        user: "bob".into(),
        hash: hash.into(),
    }];
    s
}

fn bcrypt(pw: &str) -> String {
    bcrypt::hash(pw, 4).unwrap()
}

async fn setup(site: Site) -> (burrow_web::Proxy<Fake>, std::sync::Arc<Fake>, HttpBackend) {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("grafana", app.addr);
    let proxy = start(
        |s| {
            s.sites = vec![site];
            s.forwarding.trusted_proxies = trusted();
        },
        up.clone(),
    )
    .await;
    (proxy, up, app)
}

async fn get(proxy: &burrow_web::Proxy<Fake>, extra: &str) -> String {
    over_tls(
        proxy.https_addr().unwrap(),
        "grafana.example.com",
        &format!("GET / HTTP/1.1\r\nHost: grafana.example.com\r\n{extra}Connection: close\r\n\r\n"),
    )
    .await
}

fn auth(user: &str, pw: &str) -> String {
    format!("Authorization: {}\r\n", encode(user, pw.as_bytes()))
}

fn from(ip: &str) -> String {
    format!("X-Forwarded-For: {ip}\r\n")
}

#[tokio::test]
async fn a_login_is_asked_for_and_the_app_never_sees_the_password() {
    let (proxy, up, _app) = setup(protected(&bcrypt("pa:ss"))).await;
    let r = get(&proxy, "").await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    assert!(
        r.to_ascii_lowercase()
            .contains("www-authenticate: basic realm=\"grafana\", charset=\"utf-8\"\r\n"),
        "{r}"
    );
    assert!(up.visitors.lock().unwrap().is_empty());
    let r = get(&proxy, &auth("bob", "pa:ss")).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(!r.to_ascii_lowercase().contains("\nauthorization:"), "{r}");
    let lower = auth("bob", "pa:ss").replacen("Basic", "basic", 1);
    assert!(get(&proxy, &lower).await.starts_with("HTTP/1.1 200"));
    for bad in [
        auth("bob", "pa"),
        auth("alice", "pa:ss"),
        "Authorization: Basic !!!\r\n".into(),
        "Authorization: Bearer x\r\n".into(),
    ] {
        let r = get(&proxy, &bad).await;
        assert!(r.starts_with("HTTP/1.1 401"), "{bad}: {r}");
    }
}

#[tokio::test]
async fn a_hash_burrow_passwd_writes_logs_in() {
    let hash = burrow_web::auth::hash_password(b"correct horse");
    let (proxy, _up, _app) = setup(protected(&hash)).await;
    let r = get(&proxy, &auth("bob", "correct horse")).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
}

#[tokio::test]
async fn ten_wrong_passwords_from_one_64_then_429_with_retry_after() {
    let (proxy, _up, _app) = setup(protected(&bcrypt("pw"))).await;
    for i in 0..9 {
        let r = get(
            &proxy,
            &format!(
                "{}{}",
                from(&format!("2001:db8:1:1::{i}")),
                auth("bob", "x")
            ),
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 401"), "{i}: {r}");
    }
    let r = get(
        &proxy,
        &format!("{}{}", from("2001:db8:1:1::ff"), auth("bob", "x")),
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 429"), "{r}");
    assert!(r.to_ascii_lowercase().contains("retry-after: "), "{r}");
    let right = auth("bob", "pw");
    let r = get(&proxy, &format!("{}{right}", from("2001:db8:1:1:aaaa::1"))).await;
    assert!(
        r.starts_with("HTTP/1.1 429"),
        "the right password waits too: {r}"
    );
    let r = get(&proxy, &format!("{}{right}", from("2001:db8:1:2::1"))).await;
    assert!(r.starts_with("HTTP/1.1 200"), "another /64: {r}");
}

#[tokio::test]
async fn a_denied_address_gets_403_whether_the_password_is_right_or_wrong() {
    let (proxy, up, _app) = setup(protected(&bcrypt("pw"))).await;
    up.denied
        .lock()
        .unwrap()
        .push("198.51.100.66".parse().unwrap());
    for pw in ["pw", "wrong", ""] {
        let r = get(
            &proxy,
            &format!("{}{}", from("198.51.100.66"), auth("bob", pw)),
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 403"), "{pw}: {r}");
    }
    let r = get(&proxy, &from("198.51.100.66")).await;
    assert!(r.starts_with("HTTP/1.1 403"), "{r}");
}

fn limited(rate: u32) -> Site {
    let mut s = https_site("grafana", &["grafana.example.com"]);
    s.options.rate_limit = NonZeroU32::new(rate);
    s
}

async fn burst(proxy: &burrow_web::Proxy<Fake>, ip: &str, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    for _ in 0..n {
        out.push(get(proxy, &from(ip)).await);
    }
    out
}

#[tokio::test]
async fn above_rate_plus_burst_the_visitor_gets_429_and_others_do_not() {
    let (proxy, _up, _app) = setup(limited(1)).await;
    let first = burst(&proxy, "203.0.113.5", 3).await;
    assert!(first[0].starts_with("HTTP/1.1 200"), "{}", first[0]);
    assert!(first[1].starts_with("HTTP/1.1 200"), "{}", first[1]);
    assert!(first[2].starts_with("HTTP/1.1 429"), "{}", first[2]);
    assert!(
        first[2].to_ascii_lowercase().contains("retry-after: 1\r\n"),
        "{}",
        first[2]
    );
    assert!(burst(&proxy, "::ffff:203.0.113.5", 1).await[0].starts_with("HTTP/1.1 429"));
    assert!(burst(&proxy, "203.0.113.6", 1).await[0].starts_with("HTTP/1.1 200"));
    let v6 = burst(&proxy, "2001:db8:9:9::1", 2).await;
    assert!(v6.iter().all(|r| r.starts_with("HTTP/1.1 200")));
    assert!(burst(&proxy, "2001:db8:9:9:ffff::2", 1).await[0].starts_with("HTTP/1.1 429"));
    assert!(burst(&proxy, "2001:db8:9:a::1", 1).await[0].starts_with("HTTP/1.1 200"));
}

#[tokio::test]
async fn a_reload_keeps_the_budget_unless_the_rate_changes() {
    let (proxy, _up, _app) = setup(limited(1)).await;
    let _ = burst(&proxy, "203.0.113.7", 2).await;
    let settings = |rate: u32| {
        let mut s = ProxySettings::new(proxy.https_addr().unwrap());
        s.sites = vec![limited(rate)];
        s.forwarding.trusted_proxies = trusted();
        s
    };
    proxy.apply(&settings(1)).await;
    assert!(burst(&proxy, "203.0.113.7", 1).await[0].starts_with("HTTP/1.1 429"));
    proxy.apply(&settings(2)).await;
    assert!(burst(&proxy, "203.0.113.7", 1).await[0].starts_with("HTTP/1.1 200"));
}
