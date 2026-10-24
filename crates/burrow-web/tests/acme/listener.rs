use std::sync::Arc;
use std::time::{Duration, Instant};

use burrow_web::tls::{CertCheck, CertPhase, Certificates, Store};
use burrow_web::{AcmeSettings, CertSettings, Listeners, Proxy, ProxySettings};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::rustls::{self, ClientConfig, RootCertStore};

use super::common::{Fake, http_backend, https_site};
use super::pebble::{DIRECTORY, Pebble};

fn proxy_settings(
    p: &Pebble,
    state: &std::path::Path,
    hosts: &[(&str, &str)],
    checks: &[CertCheck],
) -> ProxySettings {
    let mut s = ProxySettings::new(([127, 0, 0, 1], p.tls_port).into());
    s.http = Some(([127, 0, 0, 1], p.http_port).into());
    s.sites = hosts.iter().map(|(svc, h)| https_site(svc, &[h])).collect();
    s.certs = CertSettings {
        acme: Some(AcmeSettings {
            directory_url: DIRECTORY.into(),
            ca: Some(p.ca.clone()),
            email: "ops@example.com".into(),
            profile: None,
            state_dir: Some(state.to_path_buf()),
            challenges: checks.to_vec(),
            fallback_directory: None,
        }),
        ..CertSettings::default()
    };
    s
}

async fn get(p: &Pebble, root: &CertificateDer<'static>, name: &str) -> String {
    let mut roots = RootCertStore::empty();
    roots.add(root.clone()).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let tcp = TcpStream::connect(("127.0.0.1", p.tls_port)).await.unwrap();
    let server = ServerName::try_from(name.to_string()).unwrap();
    let Ok(mut tls) = TlsConnector::from(Arc::new(config))
        .connect(server, tcp)
        .await
    else {
        return String::new();
    };
    let req = format!("GET / HTTP/1.1\r\nHost: {name}\r\nConnection: close\r\n\r\n");
    tls.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), tls.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

async fn ok_for(store: &Store, name: &str) {
    let start = Instant::now();
    while !store
        .status()
        .iter()
        .any(|s| s.name == name && s.phase == CertPhase::Ok)
    {
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "{:#?}",
            store.status()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn bind(port: u16) -> TcpListener {
    let start = Instant::now();
    loop {
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => return l,
            Err(e) => assert!(start.elapsed() < Duration::from_secs(5), "port {port}: {e}"),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub async fn through_the_web_listener(p: &Pebble, root: &CertificateDer<'static>) {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("www", app.addr);
    up.serve("shop", app.addr);
    let state = tempfile::tempdir().unwrap();
    let store = Store::new();
    let listeners = Listeners {
        https: Some(bind(p.tls_port).await),
        http: Some(bind(p.http_port).await),
    };
    let first = proxy_settings(p, state.path(), &[("www", "w.test")], &[CertCheck::Tls]);
    let proxy = Proxy::start(listeners, first, up.clone(), store.clone()).await;
    ok_for(&store, "w.test").await;
    let r = get(p, root, "w.test").await;
    assert!(
        r.starts_with("HTTP/1.1 200"),
        "TLS-ALPN-01 on the real HTTPS port, then served: {r}"
    );
    let both = proxy_settings(
        p,
        state.path(),
        &[("www", "w.test"), ("shop", "x.test")],
        &[CertCheck::Http],
    );
    proxy.apply(&both).await;
    ok_for(&store, "x.test").await;
    let st = store
        .status()
        .into_iter()
        .find(|s| s.name == "x.test")
        .unwrap();
    assert_eq!(
        st.check,
        Some(CertCheck::Http),
        "HTTP-01 on the real plain port"
    );
    let r = get(p, root, "x.test").await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    proxy.stop().await;
}
