use std::path::Path;
use std::sync::Arc;

use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::rustls::{self, ClientConfig, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use super::*;
use crate::tls::files::tests::write_pair;
use crate::{CertSource, SharedFile};

async fn served(store: Arc<Store>, name: &str) -> Option<CertificateDer<'static>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(store);
    let client = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(crate::http::upstream::target_tls::any_certificate())
        .with_no_client_auth();
    let (a, b) = tokio::io::duplex(64 * 1024);
    let accept = TlsAcceptor::from(Arc::new(server));
    let side = tokio::spawn(async move { accept.accept(b).await.is_ok() });
    let name = ServerName::try_from(name.to_string()).unwrap();
    let tls = TlsConnector::from(Arc::new(client)).connect(name, a).await;
    let got = tls.ok().and_then(|t| {
        t.get_ref()
            .1
            .peer_certificates()
            .map(|c| c[0].clone().into_owned())
    });
    let _ = side.await;
    got
}

fn leaf(cert: &Path) -> CertificateDer<'static> {
    files::load(cert, &cert.with_extension("key")).unwrap().cert[0].clone()
}

fn named(name: &str, source: CertSource, group: &str) -> CertName {
    CertName {
        name: name.into(),
        source,
        group: group.into(),
    }
}

fn own(c: &Path, k: &Path) -> CertSource {
    CertSource::OwnFile {
        cert: c.to_path_buf(),
        key: k.to_path_buf(),
    }
}

fn shared(hosts: &[&str], c: &Path, k: &Path) -> CertSettings {
    CertSettings {
        shared: vec![SharedFile {
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            cert: c.to_path_buf(),
            key: k.to_path_buf(),
        }],
        ..CertSettings::default()
    }
}

#[tokio::test]
async fn each_name_is_served_the_file_the_selection_order_gives_it() {
    let dir = tempfile::tempdir().unwrap();
    let (oc, ok) = write_pair(dir.path(), "own", &["blog.example.com"]);
    let (sc, sk) = write_pair(dir.path(), "star", &["*.example.com", "other.example.org"]);
    let settings = shared(&["*.example.com"], &sc, &sk);
    let store = Store::new();
    store
        .apply(
            &settings,
            &[
                named("blog.example.com", own(&oc, &ok), "blog"),
                named("*.example.com", CertSource::Auto, "apps"),
                named("other.example.org", CertSource::Auto, "other"),
            ],
        )
        .await;
    assert_eq!(
        served(store.clone(), "blog.example.com").await,
        Some(leaf(&oc))
    );
    assert_eq!(
        served(store.clone(), "x.example.com").await,
        Some(leaf(&sc))
    );
    assert_eq!(
        served(store.clone(), "X.Example.com").await,
        Some(leaf(&sc))
    );
    assert_eq!(served(store.clone(), "other.example.org").await, None);
    assert_eq!(served(store.clone(), "a.b.example.com").await, None);
    assert_eq!(store.state("x.example.com"), CertState::Ready);
    assert_eq!(store.state("other.example.org"), CertState::Failing);
    let status = store.status();
    assert_eq!(status.len(), 2);
    assert_eq!(status[0].origin, CertOrigin::OwnFile { path: oc.clone() });
    assert_eq!(
        status[1].origin,
        CertOrigin::SharedFile { path: sc.clone() }
    );
    assert!(
        status
            .iter()
            .all(|s| s.phase == CertPhase::Ok && s.last_error.is_none())
    );
}

#[tokio::test]
async fn a_file_that_does_not_cover_its_name_is_never_served() {
    let dir = tempfile::tempdir().unwrap();
    let (c, k) = write_pair(dir.path(), "own", &["blog.example.com"]);
    let store = Store::new();
    store
        .apply(
            &CertSettings::default(),
            &[named("www.example.com", own(&c, &k), "blog")],
        )
        .await;
    assert_eq!(served(store.clone(), "www.example.com").await, None);
    assert_eq!(store.state("www.example.com"), CertState::Failing);
    let reason = format!("{} does not cover \"www.example.com\"", c.display());
    assert_eq!(store.status()[0].phase, CertPhase::Failing { reason });
}

#[tokio::test]
async fn a_changed_file_is_served_after_apply_and_a_broken_one_keeps_the_last_good() {
    let dir = tempfile::tempdir().unwrap();
    let (c, k) = write_pair(dir.path(), "own", &["blog.example.com"]);
    let first = leaf(&c);
    let names = [named("blog.example.com", own(&c, &k), "blog")];
    let store = Store::new();
    store.apply(&CertSettings::default(), &names).await;
    assert_eq!(
        served(store.clone(), "blog.example.com").await,
        Some(first.clone())
    );
    write_pair(dir.path(), "own", &["blog.example.com"]);
    let second = leaf(&c);
    assert_ne!(first, second);
    store.apply(&CertSettings::default(), &names).await;
    assert_eq!(
        served(store.clone(), "blog.example.com").await,
        Some(second.clone())
    );
    std::fs::write(&k, "half written").unwrap();
    store.apply(&CertSettings::default(), &names).await;
    assert_eq!(
        served(store.clone(), "blog.example.com").await,
        Some(second)
    );
    assert_eq!(store.state("blog.example.com"), CertState::Ready);
    let s = &store.status()[0];
    assert_eq!(s.phase, CertPhase::Ok);
    assert!(
        s.last_error
            .as_deref()
            .is_some_and(|e| e.contains("cannot read"))
    );
}

#[tokio::test]
async fn the_tunnel_name_reports_its_websocket_file_and_removed_names_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (c, k) = write_pair(dir.path(), "ws", &["tunnel.example.com"]);
    let store = Store::new();
    let tunnel = named(
        "tunnel.example.com",
        own(&c, &k),
        super::super::select::TUNNEL_GROUP,
    );
    store.apply(&CertSettings::default(), &[tunnel]).await;
    assert_eq!(
        store.status()[0].origin,
        CertOrigin::TunnelFile { path: c.clone() }
    );
    store.apply(&CertSettings::default(), &[]).await;
    assert!(store.status().is_empty());
    assert_eq!(served(store.clone(), "tunnel.example.com").await, None);
}

#[tokio::test]
async fn status_reports_when_a_file_expires() {
    let dir = tempfile::tempdir().unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["blog.example.com".to_string()]).unwrap();
    params.not_after = rcgen::date_time_ymd(2099, 1, 1);
    let (c, k) = (dir.path().join("a.pem"), dir.path().join("a.key"));
    std::fs::write(&c, params.self_signed(&key).unwrap().pem()).unwrap();
    std::fs::write(&k, key.serialize_pem()).unwrap();
    let store = Store::new();
    store
        .apply(
            &CertSettings::default(),
            &[named("blog.example.com", own(&c, &k), "blog")],
        )
        .await;
    assert_eq!(store.status()[0].expires, Some(4_070_908_800));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn apply_reads_files_off_the_async_workers() {
    let dir = tempfile::tempdir().unwrap();
    let (c, k) = write_pair(dir.path(), "own", &["blog.example.com"]);
    let pem = std::fs::read(&c).unwrap();
    std::fs::remove_file(&c).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&c)
            .status()
            .unwrap()
            .success()
    );
    let store = Store::new();
    let names = vec![named("blog.example.com", own(&c, &k), "blog")];
    let fifo = c.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let at = std::time::Instant::now();
        std::fs::write(fifo, pem).unwrap();
        at
    });
    let applying = {
        let store = store.clone();
        tokio::spawn(async move { store.apply(&CertSettings::default(), &names).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let ran = tokio::spawn(async { std::time::Instant::now() })
        .await
        .unwrap();
    applying.await.unwrap();
    let written = writer.join().unwrap();
    assert!(ran < written, "a task ran while the file was being read");
    assert_eq!(store.state("blog.example.com"), CertState::Ready);
}

#[tokio::test]
async fn a_file_close_to_expiry_is_warned_about_once_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["blog.example.com".to_string()]).unwrap();
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(80);
    params.not_after = now + time::Duration::days(5);
    let (c, k) = (dir.path().join("a.pem"), dir.path().join("a.key"));
    std::fs::write(&c, params.self_signed(&key).unwrap().pem()).unwrap();
    std::fs::write(&k, key.serialize_pem()).unwrap();
    let store = Store::new();
    store
        .apply(
            &CertSettings::default(),
            &[named("blog.example.com", own(&c, &k), "blog")],
        )
        .await;
    assert_eq!(store.warn_expiring(), 1, "the first daily check warns");
    assert_eq!(store.warn_expiring(), 0, "not again the same day");
}

fn acme_on(state: &Path, https: u16) -> CertSettings {
    CertSettings {
        acme: Some(crate::AcmeSettings {
            directory_url: "https://127.0.0.1:9/dir".into(),
            ca: None,
            email: "ops@example.com".into(),
            profile: None,
            state_dir: Some(state.to_path_buf()),
            check_ports: burrow_syntax::web::CheckPorts {
                https: Some(https),
                http: None,
            },
            dns: Vec::new(),
            fallback_directory: None,
        }),
        ..CertSettings::default()
    }
}

#[tokio::test]
async fn a_name_lets_encrypt_cannot_check_orders_nothing_and_says_why_unless_a_stored_one_covers_it()
 {
    let state = tempfile::tempdir().unwrap();
    let names = [named("x.example.com", CertSource::Auto, "x")];
    let store = Store::new();
    store.apply(&acme_on(state.path(), 8443), &names).await;
    assert_eq!(store.state("x.example.com"), CertState::Failing);
    let st = &store.status()[0];
    assert!(
        matches!(&st.phase, CertPhase::Failing { reason } if reason.contains("cannot be checked by Let's Encrypt: open http: 80")),
        "{st:#?}"
    );
    assert!(store.queue().is_empty(), "nothing is ordered");
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["x.example.com".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let stored = crate::tls::acme::storage::Stored {
        chain_pem: cert.pem(),
        key_pem: key.serialize_pem(),
        meta: crate::tls::acme::storage::Meta {
            directory: "https://127.0.0.1:9/dir".into(),
            names: vec!["x.example.com".into()],
            issued: 1,
        },
    };
    let dir = crate::tls::acme::storage::cert_dir(state.path(), &["x.example.com".to_string()]);
    crate::tls::acme::storage::save(&dir, &stored).unwrap();
    let again = Store::new();
    again.apply(&acme_on(state.path(), 8443), &names).await;
    assert_eq!(
        again.state("x.example.com"),
        CertState::Ready,
        "the stored one is served"
    );
    assert_eq!(again.status()[0].phase, CertPhase::Ok);
}
