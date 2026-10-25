mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use burrow_web::{Site, SiteOptions, TargetTls};
use common::*;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

struct Ca {
    params: CertificateParams,
    key: KeyPair,
    pem: String,
}

fn ca() -> Ca {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let pem = params.self_signed(&key).unwrap().pem();
    Ca { params, key, pem }
}

fn leaf(ca: Option<&Ca>, names: &[&str]) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let key = KeyPair::generate().unwrap();
    let params =
        CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
    let cert = match ca {
        Some(ca) => {
            let issuer = Issuer::new(ca.params.clone(), &ca.key);
            params.signed_by(&key, &issuer).unwrap()
        }
        None => params.self_signed(&key).unwrap(),
    };
    let der = CertificateDer::from(cert.der().to_vec());
    (der, PrivateKeyDer::try_from(key.serialize_der()).unwrap())
}

async fn https_app(cert: (CertificateDer<'static>, PrivateKeyDer<'static>)) -> SocketAddr {
    let config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.0], cert.1)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let plain = http_backend().await.addr;
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(s).await else {
                    return;
                };
                let Ok(mut app) = tokio::net::TcpStream::connect(plain).await else {
                    return;
                };
                let _ = tokio::io::copy_bidirectional(&mut tls, &mut app).await;
            });
        }
    });
    addr
}

async fn ask(app: SocketAddr, target_tls: TargetTls, target_name: &str) -> String {
    let (up, _) = Fake::new(4);
    up.serve("erp", app);
    let options = SiteOptions {
        target_tls,
        target_name: target_name.into(),
        ..SiteOptions::default()
    };
    let site = Site {
        options,
        ..https_site("erp", &["erp.example.com"])
    };
    let proxy = start(|s| s.sites = vec![site], up).await;
    over_tls(
        proxy.https_addr().unwrap(),
        "erp.example.com",
        "GET /x HTTP/1.1\r\nHost: erp.example.com\r\nConnection: close\r\n\r\n",
    )
    .await
}

fn status(r: &str) -> &str {
    r.get(9..12).unwrap_or("")
}

#[tokio::test]
async fn an_unchecked_target_accepts_a_self_signed_app_that_public_roots_refuse() {
    let app = https_app(leaf(None, &["erp.lan"])).await;
    let r = ask(app, TargetTls::Unchecked, "erp.lan").await;
    assert_eq!(status(&r), "200", "{r}");
    assert!(r.contains("\nGET /x\n"), "{r}");
    let r = ask(app, TargetTls::PublicRoots, "erp.lan").await;
    assert_eq!(status(&r), "502", "{r}");
}

#[tokio::test]
async fn a_ca_file_accepts_only_apps_it_signed_for_the_target_name() {
    let dir = tempfile::tempdir().unwrap();
    let ours = ca();
    let other = ca();
    let ours_pem = dir.path().join("ours.pem");
    let other_pem = dir.path().join("other.pem");
    std::fs::write(&ours_pem, &ours.pem).unwrap();
    std::fs::write(&other_pem, &other.pem).unwrap();
    let app = https_app(leaf(Some(&ours), &["erp.lan"])).await;
    let r = ask(app, TargetTls::Ca(ours_pem.clone()), "erp.lan").await;
    assert_eq!(status(&r), "200", "{r}");
    let r = ask(app, TargetTls::Ca(other_pem), "erp.lan").await;
    assert_eq!(status(&r), "502", "{r}");
    let r = ask(app, TargetTls::Ca(ours_pem.clone()), "other.lan").await;
    assert_eq!(status(&r), "502", "{r}");
    let r = ask(
        app,
        TargetTls::Ca(dir.path().join("missing.pem")),
        "erp.lan",
    )
    .await;
    assert_eq!(status(&r), "502", "{r}");
}

#[tokio::test]
async fn an_ip_target_needs_the_ip_in_the_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let ours = ca();
    let pem = dir.path().join("ca.pem");
    std::fs::write(&pem, &ours.pem).unwrap();
    let with_ip = https_app(leaf(Some(&ours), &["127.0.0.1"])).await;
    let r = ask(with_ip, TargetTls::Ca(pem.clone()), "127.0.0.1").await;
    assert_eq!(status(&r), "200", "{r}");
    let without = https_app(leaf(Some(&ours), &["erp.lan"])).await;
    let r = ask(without, TargetTls::Ca(pem), "127.0.0.1").await;
    assert_eq!(status(&r), "502", "{r}");
}

#[tokio::test]
async fn a_plain_http_app_behind_target_tls_is_a_clear_502() {
    let plain = http_backend().await;
    let r = ask(plain.addr, TargetTls::Unchecked, "erp.lan").await;
    assert_eq!(status(&r), "502", "{r}");
}

#[tokio::test]
async fn a_target_that_never_finishes_its_tls_handshake_gets_504_in_time() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent = l.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = l.accept().await {
            held.push(s);
        }
    });
    let (up, _) = Fake::new(4);
    up.serve("erp", silent);
    let options = SiteOptions {
        target_tls: TargetTls::Unchecked,
        target_name: "erp.lan".into(),
        response_header_timeout: std::time::Duration::from_millis(300),
        ..SiteOptions::default()
    };
    let site = Site {
        options,
        ..https_site("erp", &["erp.example.com"])
    };
    let proxy = start(|s| s.sites = vec![site], up).await;
    let started = std::time::Instant::now();
    let r = over_tls(
        proxy.https_addr().unwrap(),
        "erp.example.com",
        "GET /x HTTP/1.1\r\nHost: erp.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(status(&r), "504", "{r}");
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}
