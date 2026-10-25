use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use burrow_syntax::web::Challenge;
use burrow_web::tls::acme::dns01::wire::Reach;
use burrow_web::tls::{CertCheck, CertPhase, CertStatus, Certificates, Store, cert_names};
use burrow_web::{
    AcmeSettings, CertSource, DnsEntry, DnsProvider, ProxySettings, Secret, Site, SiteTls,
    TsigAlgorithm,
};

use super::bind::{self, ADDR, Bind, KEY, KEY_384, KEY_512, SECRET, SECRET_384, SECRET_512};
use super::cloudflare::{Mock, TOKEN};
use super::pebble::{DIRECTORY, Pebble};

fn acme(p: &Pebble, state: &Path, dns: Vec<DnsEntry>) -> AcmeSettings {
    AcmeSettings {
        directory_url: DIRECTORY.into(),
        ca: Some(p.ca.clone()),
        email: "ops@example.com".into(),
        profile: None,
        state_dir: Some(state.to_path_buf()),
        check_ports: Default::default(),
        dns,
        fallback_directory: None,
    }
}

fn entry(domains: &[&str], provider: DnsProvider, wildcards: &[&str]) -> DnsEntry {
    DnsEntry {
        domains: domains.iter().map(|d| d.to_string()).collect(),
        provider,
        wait: Duration::from_secs(30),
        resolvers: vec![ADDR.parse().unwrap()],
        wildcards: wildcards.iter().map(|w| w.to_string()).collect(),
    }
}

fn rfc2136(secret: &str) -> DnsProvider {
    rfc2136_with(KEY, secret, TsigAlgorithm::HmacSha256)
}

fn rfc2136_with(key: &str, secret: &str, algorithm: TsigAlgorithm) -> DnsProvider {
    DnsProvider::Rfc2136 {
        server: ADDR.into(),
        key_name: key.into(),
        secret: Secret::new(secret.into()),
        algorithm,
    }
}

fn reach(cf: Option<&Mock>) -> Reach {
    let mut r = Reach::default();
    r.redirect
        .insert("127.0.0.1".parse().unwrap(), ADDR.parse().unwrap());
    if let Some(m) = cf {
        r.cloudflare_api = Some(m.url.clone());
        r.api_ca = Some(m.ca.clone());
    }
    r
}

fn site(service: &str, hosts: &[&str]) -> Site {
    Site {
        tls: SiteTls::Https(CertSource::Auto),
        ..Site::passthrough(service, hosts.iter().map(|h| h.to_string()).collect())
    }
}

async fn run(settings: &ProxySettings, r: Reach) -> Arc<Store> {
    let store = Store::new();
    store.set_dns_reach(r);
    store.apply(&settings.certs, &cert_names(settings)).await;
    store
}

async fn until(store: &Store, name: &str, done: impl Fn(&CertStatus) -> bool) -> CertStatus {
    let start = Instant::now();
    loop {
        if let Some(s) = store.status().into_iter().find(|s| s.name == name)
            && done(&s)
        {
            return s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "{name}: {:#?}",
            store.status()
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn no_txt(name: &str) {
    for _ in 0..40 {
        if bind::txt(name).await.is_some_and(|v| v.is_empty()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("{name} still has TXT values: {:?}", bind::txt(name).await);
}

async fn shared_wildcard_over_rfc2136(p: &Pebble) {
    let state = tempfile::tempdir().unwrap();
    let mut ps = ProxySettings::new("127.0.0.1:443".parse().unwrap());
    ps.sites = vec![
        site("wiki", &["wiki.example.test"]),
        site("blog", &["blog.example.test"]),
    ];
    let dns = vec![entry(
        &["example.test"],
        rfc2136(SECRET),
        &["*.example.test"],
    )];
    ps.certs.acme = Some(acme(p, state.path(), dns));
    let names: Vec<String> = cert_names(&ps).into_iter().map(|n| n.name).collect();
    assert_eq!(
        names,
        vec!["*.example.test", "example.test"],
        "the sites order nothing"
    );
    let store = run(&ps, reach(None)).await;
    let s = until(&store, "*.example.test", |s| s.phase == CertPhase::Ok).await;
    assert_eq!(s.check, Some(CertCheck::Dns));
    for n in ["wiki.example.test", "blog.example.test", "example.test"] {
        assert!(
            store.certificate(n).is_some(),
            "{n} is served by the wildcard"
        );
    }
    no_txt("_acme-challenge.example.test").await;
    assert!(!state.path().join("dns-pending.json").exists());
}

async fn cname_into_a_zone_the_entry_writes(p: &Pebble) {
    let state = tempfile::tempdir().unwrap();
    let mut ps = ProxySettings::new("127.0.0.1:443".parse().unwrap());
    ps.sites = vec![site("erp", &["erp.example.test"])];
    ps.certs.choices.insert("erp".into(), Challenge::Dns);
    let dns = vec![entry(&["example.test"], rfc2136(SECRET), &[])];
    ps.certs.acme = Some(acme(p, state.path(), dns));
    let store = run(&ps, reach(None)).await;
    until(&store, "erp.example.test", |s| s.phase == CertPhase::Ok).await;
    no_txt("acme-target.example.test").await;
}

async fn cloudflare_mock(p: &Pebble) {
    let mock = Mock::start().await;
    let state = tempfile::tempdir().unwrap();
    let mut ps = ProxySettings::new("127.0.0.1:443".parse().unwrap());
    ps.sites = vec![site("lab", &["lab.example.test"])];
    ps.certs.choices.insert("lab".into(), Challenge::Dns);
    let token = DnsProvider::Cloudflare {
        token: Secret::new(TOKEN.into()),
    };
    ps.certs.acme = Some(acme(
        p,
        state.path(),
        vec![entry(&["example.test"], token, &[])],
    ));
    let store = run(&ps, reach(Some(&mock))).await;
    until(&store, "lab.example.test", |s| s.phase == CertPhase::Ok).await;
    no_txt("_acme-challenge.lab.example.test").await;
    assert!(mock.records.lock().unwrap().is_empty());
}

async fn a_wrong_secret_is_a_status_error_and_nothing_is_set(p: &Pebble) {
    let state = tempfile::tempdir().unwrap();
    let mut ps = ProxySettings::new("127.0.0.1:443".parse().unwrap());
    ps.sites = vec![site("bad", &["bad.example.test"])];
    ps.certs.choices.insert("bad".into(), Challenge::Dns);
    let wrong = "d3Jvbmctc2VjcmV0LWtleS1mb3ItdGVzdHMtMzItYg==";
    ps.certs.acme = Some(acme(
        p,
        state.path(),
        vec![entry(&["example.test"], rfc2136(wrong), &[])],
    ));
    let store = run(&ps, reach(None)).await;
    let s = until(&store, "bad.example.test", |s| {
        matches!(s.phase, CertPhase::Failing { .. })
    })
    .await;
    let why = s.last_error.unwrap_or_default();
    assert!(why.contains("(BADSIG): check the secret"), "{why}");
    no_txt("_acme-challenge.bad.example.test").await;
}

async fn keys_of_other_algorithms(p: &Pebble) {
    for (name, key, secret, alg) in [
        ("k384", KEY_384, SECRET_384, TsigAlgorithm::HmacSha384),
        ("k512", KEY_512, SECRET_512, TsigAlgorithm::HmacSha512),
    ] {
        let state = tempfile::tempdir().unwrap();
        let host = format!("{name}.example.test");
        let mut ps = ProxySettings::new("127.0.0.1:443".parse().unwrap());
        ps.sites = vec![site(name, &[host.as_str()])];
        ps.certs.choices.insert(name.into(), Challenge::Dns);
        let dns = vec![entry(
            &["example.test"],
            rfc2136_with(key, secret, alg),
            &[],
        )];
        ps.certs.acme = Some(acme(p, state.path(), dns));
        let store = run(&ps, reach(None)).await;
        until(&store, &host, |s| s.phase == CertPhase::Ok).await;
        no_txt(&format!("_acme-challenge.{host}")).await;
    }
}

pub async fn all(p: Pebble, _bind: Bind) {
    shared_wildcard_over_rfc2136(&p).await;
    cname_into_a_zone_the_entry_writes(&p).await;
    cloudflare_mock(&p).await;
    a_wrong_secret_is_a_status_error_and_nothing_is_set(&p).await;
    keys_of_other_algorithms(&p).await;
}
