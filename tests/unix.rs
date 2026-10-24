#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};

use burrow::config::settings::Service;
use burrow::policy::allowlist::{AllowList, Rule};
use burrow::wire::protocol::VisitTarget;
use burrow::wire::secret::Secret;
use burrow_config::Protocol;
use common::*;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;

fn socket_echo(dir: &Path) -> PathBuf {
    let path = dir.join("echo.sock");
    let l = UnixListener::bind(&path).unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
                let _ = w.shutdown().await;
            });
        }
    });
    path
}

fn allow(rules: &[String]) -> AllowList {
    AllowList::Rules(rules.iter().map(|r| r.parse::<Rule>().unwrap()).collect())
}

fn to_socket(path: &Path) -> Service {
    Service {
        target: format!("unix:{}", path.display()),
        ..service("127.0.0.1:1".parse().unwrap(), Protocol::Tcp)
    }
}

async fn socket_tunnel(dir: &Path, rules: Vec<String>, plaintext: bool) -> (Tunnel, PathBuf) {
    let path = socket_echo(dir);
    let secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server.insecure_plaintext = plaintext;
    let t = tunnel_with(
        server,
        vec![("db", to_socket(&path))],
        move |mut c| {
            c.allow = allow(&rules);
            c.insecure_plaintext = plaintext;
            c
        },
        secret,
    )
    .await;
    (t, path)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_public_service_reaches_a_unix_socket() {
    for plaintext in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let folder = format!("unix:{}/*", dir.path().display());
        let (t, _) = socket_tunnel(dir.path(), vec![folder], plaintext).await;
        for (len, seed) in [(1, 1), (100_000, 2), (2_000_000, 3)] {
            let data = pattern(len, seed);
            assert_eq!(
                roundtrip(t.server.services["db"], &data).await,
                data,
                "{len} bytes, plaintext {plaintext}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_refuses_sockets_outside_its_allow_list() {
    let dir = tempfile::tempdir().unwrap();
    let rules = vec![
        "127.0.0.0/8".to_string(),
        format!("unix:{}/other.sock", dir.path().display()),
    ];
    let (t, _) = socket_tunnel(dir.path(), rules, false).await;
    let data = pattern(1000, 4);
    assert_ne!(
        try_roundtrip(t.server.services["db"], &data)
            .await
            .as_deref(),
        Some(&data[..])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_symbolic_link_in_an_allowed_folder_is_refused_but_a_linked_folder_works() {
    let real = tempfile::tempdir().unwrap();
    let path = socket_echo(real.path());
    let allowed = tempfile::tempdir().unwrap();
    let planted = allowed.path().join("x.sock");
    std::os::unix::fs::symlink(&path, &planted).unwrap();
    let linked = allowed.path().join("dir");
    std::os::unix::fs::symlink(real.path(), &linked).unwrap();
    let secret = Secret::generate();
    let rules = vec![
        format!("unix:{}/*", allowed.path().display()),
        format!("unix:{}/*", linked.display()),
    ];
    let t = tunnel_with(
        server_settings(vec![secret.clone()]),
        vec![
            ("planted", to_socket(&planted)),
            ("linked", to_socket(&linked.join("echo.sock"))),
        ],
        move |mut c| {
            c.allow = allow(&rules);
            c
        },
        secret,
    )
    .await;
    let data = pattern(1000, 6);
    assert_ne!(
        try_roundtrip(t.server.services["planted"], &data)
            .await
            .as_deref(),
        Some(&data[..])
    );
    assert_eq!(roundtrip(t.server.services["linked"], &data).await, data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_visitor_reaches_a_unix_socket_on_a_client() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_echo(dir.path());
    let secret = Secret::generate();
    let visitor_secret = Secret::generate();
    let mut server = server_settings(vec![secret.clone()]);
    server.visitors.insert(
        "laptop".into(),
        visitor_auth(visitor_secret.clone(), &["home"]),
    );
    let exact = format!("unix:{}", path.display());
    let rules = vec![exact.clone()];
    let t = tunnel_with(
        server,
        vec![],
        move |mut c| {
            c.allow = allow(&rules);
            c
        },
        secret,
    )
    .await;
    let v = start_visitor(visitor_settings(
        t.server.addr,
        visitor_secret,
        vec![(
            "sock",
            VisitTarget::Client {
                client: "home".into(),
                target: exact,
            },
        )],
    ));
    let data = pattern(300_000, 5);
    assert_eq!(roundtrip(v.addr("sock"), &data).await, data);
}
