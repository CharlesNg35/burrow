use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use burrow_web::tls::{
    CertCheck, CertName, CertOrigin, CertPhase, CertState, CertStatus, Certificates, Store,
};
use burrow_web::{AcmeSettings, CertSettings, CertSource};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use super::pebble::{DIRECTORY, Pebble};
use super::responder::{Responder, start};

pub(super) struct Ctx {
    pub(super) p: Pebble,
    pub(super) r: Responder,
    pub(super) root: CertificateDer<'static>,
}

pub(super) fn settings(
    ctx: &Ctx,
    state: &Path,
    checks: &[CertCheck],
    profile: Option<&str>,
) -> CertSettings {
    CertSettings {
        acme: Some(AcmeSettings {
            directory_url: DIRECTORY.into(),
            ca: Some(ctx.p.ca.clone()),
            email: "ops@example.com".into(),
            profile: profile.map(str::to_string),
            state_dir: Some(state.to_path_buf()),
            check_ports: super::pebble::ports_of(checks),
            dns: Vec::new(),
            fallback_directory: None,
        }),
        ..CertSettings::default()
    }
}

const UNVALIDATED: &str = "never-checked.test";

fn with_a_name_pebble_never_validated(group: &str, host: &str) -> Vec<CertName> {
    names(group, &[host, UNVALIDATED])
}

pub(super) fn names(group: &str, hosts: &[&str]) -> Vec<CertName> {
    hosts
        .iter()
        .map(|h| CertName {
            name: h.to_string(),
            source: CertSource::Auto,
            group: group.into(),
        })
        .collect()
}

pub(super) async fn open_store(
    ctx: &Ctx,
    settings: &CertSettings,
    names: &[CertName],
) -> Arc<Store> {
    let store = Store::new();
    ctx.r.serve(store.clone());
    store.apply(settings, names).await;
    store
}

pub(super) fn status_of(store: &Store, name: &str) -> CertStatus {
    store
        .status()
        .into_iter()
        .find(|s| s.name == name)
        .expect("a status line")
}

pub(super) async fn until(
    store: &Store,
    name: &str,
    what: &str,
    done: impl Fn(&CertStatus) -> bool,
) -> CertStatus {
    let start = Instant::now();
    loop {
        let s = status_of(store, name);
        if done(&s) {
            return s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "{name}: waited for {what}: {s:#?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub(super) async fn served(ctx: &Ctx, name: &str) -> Option<CertificateDer<'static>> {
    let mut roots = RootCertStore::empty();
    roots.add(ctx.root.clone()).unwrap();
    let provider = burrow_web::tls::provider();
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let tcp = TcpStream::connect(("127.0.0.1", ctx.p.tls_port))
        .await
        .ok()?;
    let server = ServerName::try_from(name.to_string()).unwrap();
    let mut tls = TlsConnector::from(Arc::new(config))
        .connect(server, tcp)
        .await
        .ok()?;
    let mut ok = Vec::new();
    let _ = tls.read_to_end(&mut ok).await;
    let leaf = tls.get_ref().1.peer_certificates()?[0].clone().into_owned();
    (ok == b"ok").then_some(leaf)
}

fn a_dir(state: &Path) -> std::path::PathBuf {
    burrow_web::tls::acme::storage::cert_dir(state, &["a.test".to_string()])
}

fn stored_pem(state: &Path) -> String {
    std::fs::read_to_string(a_dir(state).join("cert.pem")).unwrap()
}

pub(super) fn stored_leaf(state: &Path) -> CertificateDer<'static> {
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    CertificateDer::from_pem_slice(stored_pem(state).as_bytes()).unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

pub(super) const PAST: &str =
    r#"{"suggestedWindow": {"start": "2020-01-01T00:00:00Z", "end": "2020-01-02T00:00:00Z"}}"#;

pub async fn all(p: Pebble) {
    let r = start(Store::new(), p.tls_port, p.http_port).await;
    let root = p.root().await;
    let ctx = Ctx { p, r, root };
    let state = tempfile::tempdir().unwrap();
    tls_alpn(&ctx, state.path()).await;
    http01(&ctx).await;
    fallback_to_http(&ctx).await;
    ip_address(&ctx).await;
    restart_offline(&ctx, state.path()).await;
    restart_and_rename_order_nothing(&ctx, state.path()).await;
    let before = tempfile::tempdir().unwrap();
    copy_dir(state.path(), before.path());
    ari_replaces(&ctx, state.path()).await;
    already_replaced(&ctx, before.path()).await;
    super::replaces::a_refused_replaces_order_is_followed_by_a_plain_order(&ctx, state.path())
        .await;
    ari_malformed_and_explained(&ctx, state.path()).await;
    failure_keeps_the_old(&ctx, state.path()).await;
    a_renewal_that_cannot_be_ordered_keeps_the_old(&ctx).await;
    profiles(&ctx).await;
    the_guard_stops_an_order_loop(&ctx).await;
    the_everyday_example_gets_its_own_certificate(&ctx).await;
    https_in_front_falls_back_to_http_at_once_and_is_remembered(&ctx).await;
    a_chosen_check_is_the_only_one_tried(&ctx).await;
    https_in_front_with_443_only_backs_off_and_keeps_the_old(&ctx).await;
    an_added_name_reorders_once_and_a_removed_name_orders_nothing(&ctx).await;
    fallback_directory(&ctx).await;
    authz_reuse(&ctx).await;
    let Ctx { p, r, .. } = ctx;
    p.run_pebble(&["PEBBLE_AUTHZREUSE=0", "PEBBLE_WFE_NONCEREJECT=50"])
        .await;
    let ctx = Ctx {
        root: p.root().await,
        p,
        r,
    };
    super::nonce::refused_nonces_are_retried(&ctx).await;
    let Ctx { p, r, .. } = ctx;
    drop(r);
    p.run_pebble(&["PEBBLE_AUTHZREUSE=0"]).await;
    let root = p.root().await;
    super::listener::through_the_web_listener(&p, &root).await;
    super::listener::a_tunnel_only_config_gets_and_renews_its_certificate(&p, &root).await;
}

async fn tls_alpn(ctx: &Ctx, state: &Path) {
    let s = settings(ctx, state, &[CertCheck::Tls], None);
    let store = open_store(ctx, &s, &names("blog", &["a.test"])).await;
    let st = until(&store, "a.test", "issued", |s| s.phase == CertPhase::Ok).await;
    assert_eq!(st.check, Some(CertCheck::Tls));
    assert_eq!(
        st.origin,
        CertOrigin::LetsEncrypt {
            directory: DIRECTORY.into()
        }
    );
    assert!(st.expires.is_some() && st.next_renewal.is_some(), "{st:#?}");
    assert_eq!(store.state("a.test"), CertState::Ready);
    assert!(
        store
            .account()
            .is_some_and(|a| a.starts_with("https://localhost:14000/"))
    );
    assert!(
        served(ctx, "a.test").await.is_some(),
        "served and checked against Pebble's root"
    );
    for f in ["cert.pem", "key.pem", "meta.json"] {
        assert!(a_dir(state).join(f).exists(), "{f}");
    }
    let accounts: Vec<_> = std::fs::read_dir(state.join("acme")).unwrap().collect();
    assert_eq!(accounts.len(), 1, "one account for the directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(a_dir(state).join("key.pem"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

async fn http01(ctx: &Ctx) {
    let state = tempfile::tempdir().unwrap();
    let s = settings(ctx, state.path(), &[CertCheck::Http], None);
    let store = open_store(ctx, &s, &names("shop", &["b.test", "c.test"])).await;
    let st = until(&store, "c.test", "issued", |s| s.phase == CertPhase::Ok).await;
    assert_eq!(st.check, Some(CertCheck::Http));
    let (b, c) = (
        served(ctx, "b.test").await.unwrap(),
        served(ctx, "c.test").await.unwrap(),
    );
    assert_eq!(b, c, "one certificate for the service's names");
}

async fn fallback_to_http(ctx: &Ctx) {
    let state = tempfile::tempdir().unwrap();
    ctx.r.refuse_tls(true);
    let s = settings(ctx, state.path(), &[CertCheck::Tls, CertCheck::Http], None);
    let store = open_store(ctx, &s, &names("wiki", &["d.test"])).await;
    let st = until(&store, "d.test", "issued", |s| s.phase == CertPhase::Ok).await;
    ctx.r.refuse_tls(false);
    assert_eq!(
        st.check,
        Some(CertCheck::Http),
        "HTTP-01 after TLS-ALPN-01 failed"
    );
}

async fn ip_address(ctx: &Ctx) {
    let state = tempfile::tempdir().unwrap();
    let s = settings(ctx, state.path(), &[CertCheck::Tls], None);
    let store = open_store(ctx, &s, &names("kiosk", &["127.0.0.1"])).await;
    let st = until(&store, "127.0.0.1", "issued", |s| s.phase == CertPhase::Ok).await;
    let leaf = served(ctx, "127.0.0.1")
        .await
        .expect("served to a client without SNI");
    let facts = burrow_web::tls::x509::facts(&leaf).unwrap();
    assert_eq!(facts.not_after - facts.not_before, 576_000 - 1, "{st:#?}");
}

async fn restart_offline(ctx: &Ctx, state: &Path) {
    let before = stored_leaf(state);
    ctx.p.pause(true).await;
    let s = settings(ctx, state, &[CertCheck::Tls], None);
    let store = open_store(ctx, &s, &names("blog", &["a.test"])).await;
    assert_eq!(
        store.state("a.test"),
        CertState::Ready,
        "served before any network call"
    );
    assert_eq!(served(ctx, "a.test").await, Some(before));
    ctx.p.pause(false).await;
}

async fn restart_and_rename_order_nothing(ctx: &Ctx, state: &Path) {
    let before = stored_leaf(state);
    let issued = std::fs::read_to_string(a_dir(state).join("meta.json")).unwrap();
    let count = burrow_web::tls::acme::storage::all(state).len();
    for group in ["blog", "blog-renamed", "another-service"] {
        let s = settings(ctx, state, &[CertCheck::Tls], None);
        let store = open_store(ctx, &s, &names(group, &["a.test"])).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let st = status_of(&store, "a.test");
        assert_eq!(st.phase, CertPhase::Ok, "{group}: {st:#?}");
        assert_eq!(served(ctx, "a.test").await, Some(before.clone()), "{group}");
    }
    assert_eq!(
        burrow_web::tls::acme::storage::all(state).len(),
        count,
        "no new certificate"
    );
    assert_eq!(
        std::fs::read_to_string(a_dir(state).join("meta.json")).unwrap(),
        issued,
        "the stored certificate was not replaced"
    );
}

async fn restart_renewing(
    ctx: &Ctx,
    state: &Path,
    answer: &str,
) -> (Arc<Store>, CertificateDer<'static>) {
    let before = stored_leaf(state);
    ctx.p.set_renewal_info(&stored_pem(state), answer).await;
    let s = settings(ctx, state, &[CertCheck::Tls], None);
    let store = open_store(ctx, &s, &names("blog", &["a.test"])).await;
    (store, before)
}

async fn ari_replaces(ctx: &Ctx, state: &Path) {
    let (store, old) = restart_renewing(ctx, state, PAST).await;
    let start = Instant::now();
    while served(ctx, "a.test").await.as_ref() == Some(&old) {
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "{:#?}",
            store.status()
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    until(&store, "a.test", "the renewal to finish", |s| {
        s.phase == CertPhase::Ok
    })
    .await;
}

async fn already_replaced(ctx: &Ctx, state: &Path) {
    let old_pem = stored_pem(state);
    let (store, _) = restart_renewing(ctx, state, PAST).await;
    let start = Instant::now();
    while stored_pem(state) == old_pem {
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "{:#?}",
            store.status()
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    until(&store, "a.test", "the renewal to finish", |s| {
        s.phase == CertPhase::Ok
    })
    .await;
}

async fn ari_malformed_and_explained(ctx: &Ctx, state: &Path) {
    let (store, old) = restart_renewing(ctx, state, "not json").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let st = status_of(&store, "a.test");
    assert_eq!(st.phase, CertPhase::Ok);
    assert_eq!(
        served(ctx, "a.test").await,
        Some(old.clone()),
        "no renewal on a malformed answer"
    );
    let now = burrow_web::tls::now_secs();
    assert!(st.next_renewal.unwrap() > now + 5 * 3600, "{st:#?}");
    let later = r#"{"suggestedWindow": {"start": "2099-01-01T00:00:00Z", "end": "2099-01-02T00:00:00Z"}, "explanationURL": "https://example.com/why"}"#;
    let (store, _) = restart_renewing(ctx, state, later).await;
    let st = until(&store, "a.test", "the explanation", |s| {
        s.explanation.is_some()
    })
    .await;
    assert_eq!(st.explanation.as_deref(), Some("https://example.com/why"));
    assert_eq!(served(ctx, "a.test").await, Some(old));
}

async fn failure_keeps_the_old(ctx: &Ctx, state: &Path) {
    ctx.r.refuse_tls(true);
    let old = stored_leaf(state);
    let s = settings(ctx, state, &[CertCheck::Tls], None);
    let store = open_store(
        ctx,
        &s,
        &with_a_name_pebble_never_validated("blog", "a.test"),
    )
    .await;
    let st = until(&store, "a.test", "a failure", |s| {
        matches!(s.phase, CertPhase::Failing { .. })
    })
    .await;
    assert!(st.last_error.is_some() && st.failures >= 1, "{st:#?}");
    assert_eq!(store.state("a.test"), CertState::Ready);
    assert_eq!(
        served(ctx, "a.test").await,
        Some(old),
        "the old certificate is still served"
    );
    ctx.r.refuse_tls(false);
}

async fn a_renewal_that_cannot_be_ordered_keeps_the_old(ctx: &Ctx) {
    let state = tempfile::tempdir().unwrap();
    let s = settings(ctx, state.path(), &[CertCheck::Tls], None);
    let store = open_store(ctx, &s, &names("kept", &["kept.test"])).await;
    until(&store, "kept.test", "issued", |s| s.phase == CertPhase::Ok).await;
    let old = served(ctx, "kept.test").await.unwrap();
    drop(store);
    let dir = burrow_web::tls::acme::storage::cert_dir(state.path(), &["kept.test".to_string()]);
    let pem = std::fs::read_to_string(dir.join("cert.pem")).unwrap();
    ctx.p.set_renewal_info(&pem, PAST).await;
    let unorderable = settings(ctx, state.path(), &[CertCheck::Tls], Some("nope"));
    let store = open_store(ctx, &unorderable, &names("kept", &["kept.test"])).await;
    let st = until(&store, "kept.test", "the renewal failure", |s| {
        matches!(s.phase, CertPhase::Failing { .. })
    })
    .await;
    let e = st.last_error.clone().unwrap();
    assert!(
        e.contains("does not offer the profile \"nope\""),
        "the renewal fails before any check, so Pebble cannot reuse an authorization: {e}"
    );
    assert!(
        st.next_renewal.unwrap() >= burrow_web::tls::now_secs() + 50,
        "the normal backoff: {st:#?}"
    );
    ctx.r.serve(store.clone());
    assert_eq!(
        served(ctx, "kept.test").await,
        Some(old),
        "the old certificate keeps serving after a failed renewal"
    );
}

async fn profiles(ctx: &Ctx) {
    let state = tempfile::tempdir().unwrap();
    let s = settings(ctx, state.path(), &[CertCheck::Tls], Some("tlsserver"));
    let store = open_store(ctx, &s, &names("api", &["e.test"])).await;
    until(&store, "e.test", "issued", |s| s.phase == CertPhase::Ok).await;
    let leaf = served(ctx, "e.test").await.unwrap();
    let f = burrow_web::tls::x509::facts(&leaf).unwrap();
    assert_eq!(
        f.not_after - f.not_before,
        3_888_000 - 1,
        "the tlsserver profile"
    );
    let s = settings(ctx, state.path(), &[CertCheck::Tls], Some("nope"));
    let store = open_store(ctx, &s, &names("other", &["f.test"])).await;
    let st = until(&store, "f.test", "a failure", |s| {
        matches!(s.phase, CertPhase::Failing { .. })
    })
    .await;
    let e = st.last_error.unwrap();
    assert!(
        e.contains("does not offer the profile \"nope\"") && e.contains("shortlived"),
        "{e}"
    );
}

async fn the_everyday_example_gets_its_own_certificate(ctx: &Ctx) {
    let state = tempfile::tempdir().unwrap();
    let s = settings(ctx, state.path(), &[CertCheck::Tls, CertCheck::Http], None);
    let store = open_store(ctx, &s, &names("shop", &["shop.test"])).await;
    let st = until(&store, "shop.test", "issued", |s| s.phase == CertPhase::Ok).await;
    assert!(
        matches!(st.check, Some(CertCheck::Tls | CertCheck::Http)),
        "{st:#?}"
    );
    assert!(served(ctx, "shop.test").await.is_some());
    let stored = burrow_web::tls::acme::storage::all(state.path());
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].1.meta.names, ["shop.test"], "its own certificate");
}

async fn https_in_front_falls_back_to_http_at_once_and_is_remembered(ctx: &Ctx) {
    let state = tempfile::tempdir().unwrap();
    let both = settings(ctx, state.path(), &[CertCheck::Tls, CertCheck::Http], None);
    ctx.r.https_in_front(true);
    let start = Instant::now();
    let store = open_store(ctx, &both, &names("cdn", &["cdn.test"])).await;
    let st = until(&store, "cdn.test", "issued", |s| s.phase == CertPhase::Ok).await;
    ctx.r.https_in_front(false);
    assert_eq!(st.check, Some(CertCheck::Http), "{st:#?}");
    assert!(
        start.elapsed() < Duration::from_secs(45),
        "at once, not after a backoff"
    );
    drop(store);
    let dir = burrow_web::tls::acme::storage::cert_dir(state.path(), &["cdn.test".to_string()]);
    let pem = std::fs::read_to_string(dir.join("cert.pem")).unwrap();
    ctx.p.set_renewal_info(&pem, PAST).await;
    let before = orders(state.path());
    let store = open_store(ctx, &both, &names("cdn", &["cdn.test"])).await;
    let start = Instant::now();
    while orders(state.path()) == before {
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "{:#?}",
            store.status()
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let st = until(&store, "cdn.test", "renewed", |s| s.phase == CertPhase::Ok).await;
    assert_eq!(
        st.check,
        Some(CertCheck::Http),
        "the remembered check is tried first"
    );
}

async fn a_chosen_check_is_the_only_one_tried(ctx: &Ctx) {
    use burrow_syntax::web::Challenge;
    for (choice, want) in [
        (Challenge::Http, CertCheck::Http),
        (Challenge::Tls, CertCheck::Tls),
    ] {
        let state = tempfile::tempdir().unwrap();
        let mut s = settings(ctx, state.path(), &[CertCheck::Tls, CertCheck::Http], None);
        s.choices.insert("chosen".into(), choice);
        ctx.r.https_in_front(choice == Challenge::Http);
        let store = open_store(ctx, &s, &names("chosen", &["chosen.test"])).await;
        let st = until(&store, "chosen.test", "issued", |s| {
            s.phase == CertPhase::Ok
        })
        .await;
        ctx.r.https_in_front(false);
        assert_eq!(st.check, Some(want), "{st:#?}");
        assert_eq!(st.failures, 0, "{choice:?}: no other check was tried first");
    }
}

async fn https_in_front_with_443_only_backs_off_and_keeps_the_old(ctx: &Ctx) {
    let state = tempfile::tempdir().unwrap();
    let tls_only = settings(ctx, state.path(), &[CertCheck::Tls], None);
    let store = open_store(ctx, &tls_only, &names("wiki", &["wiki.test"])).await;
    until(&store, "wiki.test", "issued", |s| s.phase == CertPhase::Ok).await;
    let old = served(ctx, "wiki.test").await.unwrap();
    drop(store);
    ctx.r.https_in_front(true);
    let renew = with_a_name_pebble_never_validated("wiki", "wiki.test");
    let store = open_store(ctx, &tls_only, &renew).await;
    let st = until(&store, "wiki.test", "the blocked check", |s| {
        matches!(s.phase, CertPhase::Failing { .. })
    })
    .await;
    ctx.r.https_in_front(false);
    let e = st.last_error.clone().unwrap();
    assert!(
        e.contains("ALPN") && e.contains("acme-tls/1"),
        "the CA's text: {e}"
    );
    assert!(
        e.contains(&format!("something in front of this server answers HTTPS for {UNVALIDATED} (a CDN like Cloudflare?); open http: 80 and use challenge: http, or use challenge: dns")),
        "the name Pebble must validate is the one blocked (Pebble reuses a valid authorization 1 time in 100 even with PEBBLE_AUTHZREUSE=0): {e}"
    );
    let now = burrow_web::tls::now_secs();
    assert!(
        st.next_renewal.unwrap() >= now + 50,
        "the normal backoff: {st:#?}"
    );
    ctx.r.serve(store.clone());
    assert_eq!(
        served(ctx, "wiki.test").await,
        Some(old),
        "the old certificate keeps serving"
    );
}

pub(super) fn orders(state: &Path) -> usize {
    use burrow_web::tls::acme::guard;
    guard::load(&guard::path(state, DIRECTORY)).len()
}

async fn an_added_name_reorders_once_and_a_removed_name_orders_nothing(ctx: &Ctx) {
    let state = tempfile::tempdir().unwrap();
    let s = settings(ctx, state.path(), &[CertCheck::Tls], None);
    let store = open_store(ctx, &s, &names("blog", &["n1.test"])).await;
    until(&store, "n1.test", "issued", |s| s.phase == CertPhase::Ok).await;
    let first = served(ctx, "n1.test").await.unwrap();
    assert_eq!(orders(state.path()), 1);
    store
        .apply(&s, &names("blog", &["n1.test", "n2.test"]))
        .await;
    assert_eq!(
        store.state("n2.test"),
        CertState::Pending,
        "the new name shows the pending page"
    );
    assert_eq!(
        served(ctx, "n1.test").await,
        Some(first.clone()),
        "the old one serves meanwhile"
    );
    until(&store, "n2.test", "the new name's order", |s| {
        s.phase == CertPhase::Ok
    })
    .await;
    let both = served(ctx, "n2.test").await.unwrap();
    assert_eq!(
        served(ctx, "n1.test").await,
        Some(both.clone()),
        "one certificate for all names"
    );
    assert_ne!(both, first);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(orders(state.path()), 2, "the added name re-ordered once");
    assert!(
        burrow_web::tls::acme::guard::load(&burrow_web::tls::acme::guard::path(
            state.path(),
            DIRECTORY
        ))
        .iter()
        .all(|i| !i.ari),
        "an added name mid-life is counted by the guard"
    );
    store.apply(&s, &names("blog", &["n1.test"])).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(orders(state.path()), 2, "a removed name orders nothing");
    assert_eq!(
        served(ctx, "n1.test").await,
        Some(both),
        "until the next renewal"
    );
    assert!(served(ctx, "n2.test").await.is_none());
}

async fn the_guard_stops_an_order_loop(ctx: &Ctx) {
    use burrow_web::tls::acme::{guard, storage};
    let state = tempfile::tempdir().unwrap();
    let j = vec!["j.test".to_string()];
    let path = guard::path(state.path(), DIRECTORY);
    let now = burrow_web::tls::now_secs();
    for i in 0..4 {
        guard::record(&path, &j, false, now - 3600 + i).unwrap();
    }
    let s = settings(ctx, state.path(), &[CertCheck::Tls], None);
    let store = open_store(ctx, &s, &names("looping", &["j.test"])).await;
    let st = until(&store, "j.test", "the guard", |s| {
        matches!(s.phase, CertPhase::Failing { .. })
    })
    .await;
    let e = st.last_error.clone().unwrap();
    assert!(e.contains("burrow orders the next one after"), "{e}");
    assert!(st.next_renewal.unwrap() >= now + 6 * 24 * 3600, "{st:#?}");
    assert!(storage::all(state.path()).is_empty(), "nothing was ordered");
    assert_eq!(guard::load(&path).len(), 4);
}

async fn fallback_directory(ctx: &Ctx) {
    let state = tempfile::tempdir().unwrap();
    let mut s = settings(ctx, state.path(), &[CertCheck::Tls], None);
    let acme = s.acme.as_mut().unwrap();
    acme.directory_url = "https://localhost:1/dir".into();
    acme.fallback_directory = Some(DIRECTORY.into());
    let store = open_store(ctx, &s, &names("site", &["g.test"])).await;
    let st = until(&store, "g.test", "the test service's answer", |s| {
        s.last_error
            .as_deref()
            .is_some_and(|e| e.contains("accepted the same checks"))
    })
    .await;
    assert_eq!(store.state("g.test"), CertState::Failing, "{st:#?}");
    assert!(
        served(ctx, "g.test").await.is_none(),
        "a test certificate is never served"
    );
}

async fn authz_reuse(ctx: &Ctx) {
    ctx.p.run_pebble(&["PEBBLE_AUTHZREUSE=100"]).await;
    let state = tempfile::tempdir().unwrap();
    let s = settings(ctx, state.path(), &[CertCheck::Tls], None);
    let first = open_store(ctx, &s, &names("one", &["h.test"])).await;
    until(&first, "h.test", "issued", |s| s.phase == CertPhase::Ok).await;
    drop(first);
    ctx.r.refuse_tls(true);
    let s = settings(ctx, state.path(), &[CertCheck::Tls], None);
    let second = open_store(ctx, &s, &names("two", &["h.test"])).await;
    until(
        &second,
        "h.test",
        "issued from the reused authorization",
        |s| s.phase == CertPhase::Ok,
    )
    .await;
    ctx.r.refuse_tls(false);
}
