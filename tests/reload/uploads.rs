use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use burrow::app::reload::{Nudge, ReloadOutcome};
use burrow::config::{Loaded, Role, load_file};
use burrow::server::Server;
use burrow::wire::secret::Secret;
use burrow_web::tls::uploads::{Kind, id_of};
use tokio::sync::{mpsc, oneshot, watch};

use super::common::{free_port, server_settings, start_server, wait_for, ws_mode};

fn old_file(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
}

fn pair_for(name: &str) -> String {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec![name.to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    format!("{}{}", cert.pem(), key.serialize_pem())
}

fn config(path: &Path, ports: (u16, u16, u16), tls: &str) {
    let text = format!(
        "listen: 127.0.0.1:{}\nclients:\n  home:\n    secret: {}\nweb:\n  https: 127.0.0.1:{}\n  http: 127.0.0.1:{}\nservices:\n  site:\n    client: home\n    target: 127.0.0.1:8080\n    web: {{ hosts: [a.example.com], tls: {tls} }}\n",
        ports.0,
        Secret::generate().encode(),
        ports.1,
        ports.2
    );
    burrow::base::fs::write_private(path, &text).unwrap();
}

async fn nudge(tx: &mpsc::Sender<Nudge>) -> ReloadOutcome {
    let (reply, back) = oneshot::channel();
    tx.send(Nudge { reply }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), back)
        .await
        .expect("the reload answers within 30 s")
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_the_check_rejects_keeps_the_upload_only_the_new_config_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    burrow::install::choose_state_dir(state.clone());
    let pem = pair_for("a.example.com");
    let id = id_of(Kind::Pair, pem.as_bytes());
    let upload: PathBuf = state.join("files").join(format!("{id}.pem"));
    old_file(&upload, &pem);
    let path = dir.path().join("server.yaml");
    let ports = (free_port(), free_port(), free_port());
    config(&path, ports, &format!("{{ upload: {id} }}"));
    let env = burrow::config::env::Env::default();
    let outcome = load_file(&path, Some(Role::Server), &env).unwrap();
    let report = outcome
        .report
        .render_text(burrow::base::style::Style { color: false });
    let Some(Loaded::Server(settings, _)) = outcome.loaded else {
        panic!("{report}")
    };
    let server = Server::bind(*settings.clone()).await.unwrap();
    let refuse = Arc::new(AtomicBool::new(true));
    let refusing = refuse.clone();
    let check: burrow::app::reload::Check<burrow::config::settings::ServerSettings> =
        Box::new(move |s| {
            if refusing.load(Ordering::SeqCst) {
                Err("refused by the test".to_string())
            } else {
                Ok(s)
            }
        });
    let (reload_tx, reload_rx) = mpsc::channel(4);
    let (nudge_tx, nudge_rx) = mpsc::channel(4);
    tokio::spawn(burrow::app::reload::follow_nudged(
        path.clone(),
        *settings,
        env,
        check,
        reload_tx,
        Some(nudge_rx),
    ));
    let (stop, stop_rx) = watch::channel(false);
    tokio::spawn(server.run(reload_rx, stop_rx));

    config(&path, ports, "off");
    assert!(matches!(nudge(&nudge_tx).await, ReloadOutcome::Rejected(_)));
    assert!(upload.exists(), "a rejected reload removed an upload");

    refuse.store(false, Ordering::SeqCst);
    assert_eq!(nudge(&nudge_tx).await, ReloadOutcome::Sent);
    wait_for(|| !upload.exists()).await;
    let _ = stop.send(true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_the_server_refuses_keeps_the_upload_only_the_new_settings_dropped() {
    if ws_mode() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let upload = dir.path().join("files").join("pair-00000000000000aa.pem");
    old_file(&upload, "x");
    let mut s = server_settings(vec![Secret::generate()]);
    let listen: std::net::SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    s.listen = Some(listen);
    let http = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    let mut web = burrow::config::settings::WebSettings::on((None, Some(http)));
    web.state_dir = Some(dir.path().to_path_buf());
    web.uploads = vec!["pair-00000000000000aa".into()];
    s.web = Some(web);
    let srv = start_server(s.clone()).await;

    let mut refused = s.clone();
    refused.listen = Some(format!("127.0.0.1:{}", free_port()).parse().unwrap());
    refused.metrics = Some(listen);
    refused.web.as_mut().unwrap().uploads.clear();
    let mut applies = srv.shared.applies();
    srv.reload(refused).await;
    tokio::time::timeout(Duration::from_secs(30), applies.changed())
        .await
        .expect("the server answers the reload within 30 s")
        .unwrap();
    assert!(upload.exists(), "a refused reload removed an upload");

    let mut dropped = s;
    dropped.web.as_mut().unwrap().uploads.clear();
    srv.reload(dropped).await;
    wait_for(|| !upload.exists()).await;
    let _ = srv.stop.send(true);
}
