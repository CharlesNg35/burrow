mod common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use burrow::app::reload::{accept, follow};
use burrow::config::env::Env;
use burrow::config::env::ProcessEnv;
use burrow::config::{Loaded, Role, load_file};
use burrow::server::Server;
use burrow::wire::secret::Secret;
use burrow::wire::transport::tls;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};

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

async fn eventually_trusts(addr: SocketAddr, cert_pem: &str) -> bool {
    for _ in 0..50 {
        if trusts(addr, cert_pem).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn write_config(config: &Path, port: u16, cert: &Path, key: &Path) {
    std::fs::write(
        config,
        format!(
            "websocket:\n  listen: 127.0.0.1:{port}\n  tls:\n    cert: {}\n    key: {}\nclients:\n  home:\n    secret: {}\n",
            cert.display(),
            key.display(),
            Secret::generate().encode()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(config, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

async fn serve(config: &Path) -> SocketAddr {
    let outcome = load_file(config, Some(Role::Server), &ProcessEnv).unwrap();
    let report = outcome
        .report
        .render_text(burrow::base::style::Style { color: false });
    let Some(Loaded::Server(settings, _)) = outcome.loaded else {
        panic!("server config loads: {report}")
    };
    let server = Server::bind(*settings.clone()).await.unwrap();
    let addr = server.websocket_addr().unwrap();
    let (reload_tx, reload_rx) = mpsc::channel(4);
    let (stop_tx, stop) = watch::channel(false);
    std::mem::forget(stop_tx);
    tokio::spawn(follow(
        config.to_path_buf(),
        *settings,
        Env::default(),
        accept(),
        reload_tx,
    ));
    tokio::spawn(server.run(reload_rx, stop));
    tokio::time::sleep(Duration::from_millis(300)).await;
    addr
}

use common::free_port;

fn pair(dir: &Path, cert: &str, key: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("tls.crt"), cert).unwrap();
    std::fs::write(dir.join("tls.key"), key).unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kubernetes_secret_volume_rotation_is_picked_up() {
    use std::os::unix::fs::symlink;
    common::logs();
    let dir = tempfile::tempdir().unwrap();
    let mount = dir.path().join("tls");
    let common::Cert {
        cert_pem: old_cert,
        key_pem: old_key,
    } = common::cert(&["localhost"]);
    pair(&mount.join("..2026_01"), &old_cert, &old_key);
    symlink("..2026_01", mount.join("..data")).unwrap();
    symlink("..data/tls.crt", mount.join("tls.crt")).unwrap();
    symlink("..data/tls.key", mount.join("tls.key")).unwrap();
    let config = dir.path().join("server.yaml");
    write_config(
        &config,
        free_port(),
        &mount.join("tls.crt"),
        &mount.join("tls.key"),
    );
    let addr = serve(&config).await;
    assert!(trusts(addr, &old_cert).await);

    let common::Cert {
        cert_pem: new_cert,
        key_pem: new_key,
    } = common::cert(&["localhost"]);
    pair(&mount.join("..2026_02"), &new_cert, &new_key);
    symlink("..2026_02", mount.join("..data_tmp")).unwrap();
    std::fs::rename(mount.join("..data_tmp"), mount.join("..data")).unwrap();
    assert!(
        eventually_trusts(addr, &new_cert).await,
        "the rotated secret was not picked up"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_letsencrypt_renewal_through_live_symlinks_is_picked_up() {
    use std::os::unix::fs::symlink;
    common::logs();
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive/example");
    let live = dir.path().join("live/example");
    std::fs::create_dir_all(&archive).unwrap();
    std::fs::create_dir_all(&live).unwrap();
    let common::Cert {
        cert_pem: old_cert,
        key_pem: old_key,
    } = common::cert(&["localhost"]);
    std::fs::write(archive.join("fullchain1.pem"), &old_cert).unwrap();
    std::fs::write(archive.join("privkey1.pem"), &old_key).unwrap();
    symlink(
        "../../archive/example/fullchain1.pem",
        live.join("fullchain.pem"),
    )
    .unwrap();
    symlink(
        "../../archive/example/privkey1.pem",
        live.join("privkey.pem"),
    )
    .unwrap();
    let config = dir.path().join("server.yaml");
    write_config(
        &config,
        free_port(),
        &live.join("fullchain.pem"),
        &live.join("privkey.pem"),
    );
    let addr = serve(&config).await;
    assert!(trusts(addr, &old_cert).await);

    let common::Cert {
        cert_pem: new_cert,
        key_pem: new_key,
    } = common::cert(&["localhost"]);
    std::fs::write(archive.join("fullchain2.pem"), &new_cert).unwrap();
    std::fs::write(archive.join("privkey2.pem"), &new_key).unwrap();
    for (link, target) in [
        ("fullchain.pem", "../../archive/example/fullchain2.pem"),
        ("privkey.pem", "../../archive/example/privkey2.pem"),
    ] {
        std::fs::remove_file(live.join(link)).unwrap();
        symlink(target, live.join(link)).unwrap();
    }
    assert!(eventually_trusts(addr, &new_cert).await);
    assert!(!trusts(addr, &old_cert).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_half_rotated_or_deleted_pair_keeps_the_working_certificate() {
    common::logs();
    let dir = tempfile::tempdir().unwrap();
    let common::Cert {
        cert_pem: old_cert,
        key_pem: old_key,
    } = common::cert(&["localhost"]);
    pair(dir.path(), &old_cert, &old_key);
    let config = dir.path().join("server.yaml");
    let (cert, key) = (dir.path().join("tls.crt"), dir.path().join("tls.key"));
    write_config(&config, free_port(), &cert, &key);
    let addr = serve(&config).await;
    assert!(trusts(addr, &old_cert).await);

    let common::Cert {
        cert_pem: new_cert,
        key_pem: new_key,
    } = common::cert(&["localhost"]);
    std::fs::write(&cert, &new_cert).unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        trusts(addr, &old_cert).await,
        "a mismatched pair is not used"
    );
    std::fs::remove_file(&key).unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(trusts(addr, &old_cert).await, "a missing key is not used");
    std::fs::write(&key, "-----BEGIN PRIVATE KEY-----\nAAAA").unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(trusts(addr, &old_cert).await, "a partial key is not used");
    std::fs::write(&key, &new_key).unwrap();
    assert!(eventually_trusts(addr, &new_cert).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn certificates_moved_to_another_folder_are_watched_there() {
    common::logs();
    let dir = tempfile::tempdir().unwrap();
    let common::Cert {
        cert_pem: first_cert,
        key_pem: first_key,
    } = common::cert(&["localhost"]);
    let a: PathBuf = dir.path().join("a");
    let b: PathBuf = dir.path().join("b");
    pair(&a, &first_cert, &first_key);
    let config = dir.path().join("server.yaml");
    let port = free_port();
    write_config(&config, port, &a.join("tls.crt"), &a.join("tls.key"));
    let addr = serve(&config).await;
    assert!(trusts(addr, &first_cert).await);

    let common::Cert {
        cert_pem: second_cert,
        key_pem: second_key,
    } = common::cert(&["localhost"]);
    pair(&b, &second_cert, &second_key);
    let text = std::fs::read_to_string(&config)
        .unwrap()
        .replace(&a.display().to_string(), &b.display().to_string());
    std::fs::write(&config, text).unwrap();
    assert!(eventually_trusts(addr, &second_cert).await);

    let common::Cert {
        cert_pem: third_cert,
        key_pem: third_key,
    } = common::cert(&["localhost"]);
    let tmp = b.join("tls.crt.tmp");
    std::fs::write(&tmp, &third_cert).unwrap();
    std::fs::rename(&tmp, b.join("tls.crt")).unwrap();
    let tmp = b.join("tls.key.tmp");
    std::fs::write(&tmp, &third_key).unwrap();
    std::fs::rename(&tmp, b.join("tls.key")).unwrap();
    assert!(eventually_trusts(addr, &third_cert).await);
}

fn replace(path: &Path, text: &str) {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renewed_certificate_is_used_without_a_restart() {
    common::logs();
    let dir = tempfile::tempdir().unwrap();
    let common::Cert {
        cert_pem: old_cert,
        key_pem: old_key,
    } = common::cert(&["localhost"]);
    std::fs::write(dir.path().join("cert.pem"), &old_cert).unwrap();
    std::fs::write(dir.path().join("key.pem"), &old_key).unwrap();
    let config = dir.path().join("server.yaml");
    let port = common::free_port();
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
        .render_text(burrow::base::style::Style { color: false });
    let Some(Loaded::Server(settings, _)) = outcome.loaded else {
        panic!("server config loads: {report}")
    };
    let server = Server::bind(*settings.clone()).await.unwrap();
    let addr = server.websocket_addr().unwrap();
    let (reload_tx, reload_rx) = mpsc::channel(4);
    let (_stop_tx, stop) = watch::channel(false);
    tokio::spawn(follow(
        config.clone(),
        *settings,
        Env::default(),
        accept(),
        reload_tx,
    ));
    tokio::spawn(server.run(reload_rx, stop));
    assert!(trusts(addr, &old_cert).await);

    let common::Cert {
        cert_pem: new_cert,
        key_pem: new_key,
    } = common::cert(&["localhost"]);
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

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switching_a_release_folder_symlink_reloads_the_config_inside_it() {
    use std::os::unix::fs::symlink;
    common::logs();
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut certs = Vec::new();
    for rel in ["rel1", "rel2"] {
        let folder = dir.path().join(rel);
        let common::Cert {
            cert_pem: cert,
            key_pem: key,
        } = common::cert(&["localhost"]);
        pair(&folder, &cert, &key);
        write_config(
            &folder.join("server.yaml"),
            port,
            &folder.join("tls.crt"),
            &folder.join("tls.key"),
        );
        certs.push(cert);
    }
    symlink("rel1", dir.path().join("current")).unwrap();
    let addr = serve(&dir.path().join("current/server.yaml")).await;
    assert!(trusts(addr, &certs[0]).await);
    symlink("rel2", dir.path().join("current.new")).unwrap();
    std::fs::rename(dir.path().join("current.new"), dir.path().join("current")).unwrap();
    assert!(
        eventually_trusts(addr, &certs[1]).await,
        "switching the release folder did not reload"
    );
}
