#![allow(dead_code)]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use burrow::allowlist::{AllowList, Rule};
use burrow::client::{Client, State};
use burrow::config::model::{Cipher, Protocol};
use burrow::config::settings::{ClientAuth, ClientSettings, ServerSettings, Service, SocketTuning};
use burrow::secret::Secret;
use burrow::server::Server;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

pub fn tuning() -> SocketTuning {
    SocketTuning {
        nodelay: true,
        keepalive: Some(Duration::from_secs(30)),
    }
}

pub fn server_settings(secrets: Vec<Secret>) -> ServerSettings {
    let mut clients = BTreeMap::new();
    clients.insert(
        "home".to_string(),
        ClientAuth {
            secrets,
            max_connections: None,
        },
    );
    ServerSettings {
        listen: "127.0.0.1:0".parse().unwrap(),
        clients,
        services: BTreeMap::new(),
        insecure_plaintext: false,
        heartbeat_interval: Duration::from_secs(1),
        handshake_timeout: Duration::from_secs(2),
        max_handshakes: 64,
        max_connections_per_client: 4096,
        max_pending: 1024,
        pool_min_idle: 2,
        pool_max_idle: 64,
        tcp: tuning(),
        udp_session_timeout: Duration::from_secs(30),
    }
}

pub fn service(target: SocketAddr, protocol: Protocol) -> Service {
    Service {
        client: "home".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        target: target.to_string(),
        protocol,
        max_connections: None,
    }
}

pub fn client_settings(server: SocketAddr, secret: Secret) -> ClientSettings {
    ClientSettings {
        server: server.to_string(),
        name: "home".into(),
        secret,
        allow: AllowList::Rules(vec!["127.0.0.0/8".parse::<Rule>().unwrap()]),
        insecure_plaintext: false,
        cipher: Cipher::Chachapoly,
        proxy: None,
        reconnect_initial: Duration::from_millis(50),
        reconnect_max: Duration::from_millis(500),
        connect_timeout: Duration::from_secs(2),
        tcp: tuning(),
        udp_session_timeout: Duration::from_secs(30),
    }
}

pub struct RunningServer {
    pub addr: SocketAddr,
    pub services: BTreeMap<String, SocketAddr>,
    pub reload: mpsc::Sender<ServerSettings>,
    pub stop: watch::Sender<bool>,
    pub task: JoinHandle<()>,
    pub shared: std::sync::Arc<burrow::server::Shared>,
}

pub async fn start_server(settings: ServerSettings) -> RunningServer {
    let names: Vec<String> = settings.services.keys().cloned().collect();
    let server = Server::bind(settings).await.expect("server binds");
    let addr = server.local_addr();
    let services = names
        .iter()
        .map(|n| (n.clone(), server.service_addr(n).unwrap()))
        .collect();
    let shared = server.shared();
    let (reload, reload_rx) = mpsc::channel(4);
    let (stop, stop_rx) = watch::channel(false);
    let task = tokio::spawn(server.run(reload_rx, stop_rx));
    RunningServer {
        addr,
        services,
        reload,
        stop,
        task,
        shared,
    }
}

pub struct RunningClient {
    pub state: watch::Receiver<State>,
    pub stop: watch::Sender<bool>,
    pub task: JoinHandle<()>,
}

pub async fn start_client(settings: ClientSettings) -> RunningClient {
    let client = Client::new(settings);
    let state = client.state();
    let (stop, stop_rx) = watch::channel(false);
    let task = tokio::spawn(client.run(stop_rx));
    RunningClient { state, stop, task }
}

pub async fn wait_state(state: &mut watch::Receiver<State>, want: State) {
    tokio::time::timeout(Duration::from_secs(10), state.wait_for(|s| *s == want))
        .await
        .expect("state reached in time")
        .expect("client alive");
}

pub async fn wait_for<F: FnMut() -> bool>(mut f: F) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("condition not reached in time");
}

pub async fn echo_server() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
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
    addr
}

pub async fn udp_echo_server() -> SocketAddr {
    let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = s.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            let Ok((n, from)) = s.recv_from(&mut buf).await else {
                return;
            };
            let _ = s.send_to(&buf[..n], from).await;
        }
    });
    addr
}

pub async fn roundtrip(addr: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (mut r, mut w) = s.into_split();
    let data = payload.to_vec();
    let writer = tokio::spawn(async move {
        w.write_all(&data).await.unwrap();
        w.shutdown().await.unwrap();
    });
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(60), r.read_to_end(&mut out))
        .await
        .expect("roundtrip in time")
        .unwrap();
    writer.await.unwrap();
    out
}

pub async fn try_roundtrip(addr: SocketAddr, payload: &[u8]) -> Option<Vec<u8>> {
    let mut s = tokio::net::TcpStream::connect(addr).await.ok()?;
    s.write_all(payload).await.ok()?;
    s.shutdown().await.ok()?;
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), s.read_to_end(&mut out))
        .await
        .ok()?
        .ok()?;
    Some(out)
}

pub struct Tunnel {
    pub server: RunningServer,
    pub client: RunningClient,
    pub secret: Secret,
}

pub async fn tunnel_with(
    mut server: ServerSettings,
    services: Vec<(&str, Service)>,
    client: impl FnOnce(ClientSettings) -> ClientSettings,
    secret: Secret,
) -> Tunnel {
    for (n, s) in services {
        server.services.insert(n.to_string(), s);
    }
    let srv = start_server(server).await;
    let mut cli = start_client(client(client_settings(srv.addr, secret.clone()))).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    Tunnel {
        server: srv,
        client: cli,
        secret,
    }
}

pub async fn tunnel(services: Vec<(&str, Service)>) -> Tunnel {
    let secret = Secret::generate();
    tunnel_with(
        server_settings(vec![secret.clone()]),
        services,
        |c| c,
        secret,
    )
    .await
}

pub fn pattern(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}
