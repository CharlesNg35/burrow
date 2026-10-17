mod common;

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use burrow::config::env::ProcessEnv;
use burrow::config::{Loaded, Role, load_file};
use burrow::run::{Env, reloader};
use burrow::secret::Secret;
use burrow::server::Server;
use burrow::transport::tls;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};

fn self_signed() -> (String, String) {
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    let cert = params.self_signed(&key).unwrap();
    (cert.pem(), key.serialize_pem())
}

fn replace(path: &Path, text: &str) {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

async fn trusts(addr: SocketAddr, cert_pem: &str) -> bool {
    let connector = tls::connector(Some(cert_pem.as_bytes())).unwrap();
    let Ok(tcp) = TcpStream::connect(addr).await else {
        return false;
    };
    let name = tls::server_name("localhost").unwrap();
    tokio::time::timeout(Duration::from_secs(2), connector.connect(name, tcp))
        .await
        .is_ok_and(|r| r.is_ok())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renewed_certificate_is_used_without_a_restart() {
    common::logs();
    let dir = tempfile::tempdir().unwrap();
    let (old_cert, old_key) = self_signed();
    std::fs::write(dir.path().join("cert.pem"), &old_cert).unwrap();
    std::fs::write(dir.path().join("key.pem"), &old_key).unwrap();
    let config = dir.path().join("server.yaml");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::write(
        &config,
        format!(
            "websocket:\n  listen: 127.0.0.1:{port}\n  tls:\n    cert: cert.pem\n    key: key.pem\nclients:\n  home:\n    secret: {}\n",
            Secret::generate().encode()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let outcome = load_file(&config, Some(Role::Server), &ProcessEnv).unwrap();
    let report = outcome
        .report
        .render_text(burrow::diagnostics::Style { color: false });
    let Some(Loaded::Server(settings, _)) = outcome.loaded else {
        panic!("server config loads: {report}")
    };
    let server = Server::bind(*settings.clone()).await.unwrap();
    let addr = server.websocket_addr().unwrap();
    let (reload_tx, reload_rx) = mpsc::channel(4);
    let (_stop_tx, stop) = watch::channel(false);
    tokio::spawn(reloader(
        config.clone(),
        *settings,
        Env::default(),
        reload_tx,
    ));
    tokio::spawn(server.run(reload_rx, stop));
    assert!(trusts(addr, &old_cert).await);

    let (new_cert, new_key) = self_signed();
    tokio::time::sleep(Duration::from_millis(300)).await;
    replace(&dir.path().join("cert.pem"), &new_cert);
    replace(&dir.path().join("key.pem"), &new_key);
    for _ in 0..100 {
        if trusts(addr, &new_cert).await {
            assert!(
                !trusts(addr, &old_cert).await,
                "the old certificate is gone"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the renewed certificate was not picked up");
}
