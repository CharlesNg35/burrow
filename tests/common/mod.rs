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
use tokio::net::UdpSocket;
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
        listen: Some("127.0.0.1:0".parse().unwrap()),
        websocket: None,
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
        endpoint: burrow::transport::endpoint::Endpoint::Tcp {
            host_port: server.to_string(),
        },
        headers: Vec::new(),
        ca_pem: None,
        name: "home".into(),
        secret,
        allow: AllowList::Rules(vec!["127.0.0.0/8".parse::<Rule>().unwrap()]),
        insecure_plaintext: false,
        cipher: Cipher::Chachapoly,
        proxy: None,
        reconnect_initial: Duration::from_millis(50),
        reconnect_max: Duration::from_millis(500),
        give_up_after: None,
        exit_on_rejected: false,
        connect_timeout: Duration::from_secs(2),
        tcp: tuning(),
        udp_session_timeout: Duration::from_secs(30),
    }
}

pub struct RunningServer {
    pub addr: SocketAddr,
    pub ws_addr: Option<SocketAddr>,
    pub services: BTreeMap<String, SocketAddr>,
    pub reload_tx: mpsc::Sender<ServerSettings>,
    pub ws_settings: Option<burrow::config::settings::WebSocketSettings>,
    pub stop: watch::Sender<bool>,
    pub task: JoinHandle<()>,
    pub shared: std::sync::Arc<burrow::server::Shared>,
}

pub fn ws_mode() -> bool {
    std::env::var("BURROW_TEST_TRANSPORT").is_ok_and(|v| v == "ws")
}

fn as_websocket(mut settings: ServerSettings) -> ServerSettings {
    if ws_mode() && settings.websocket.is_none() {
        let listen = settings
            .listen
            .take()
            .unwrap_or_else(|| "127.0.0.1:0".parse().unwrap());
        settings.websocket = Some(burrow::config::settings::WebSocketSettings {
            listen,
            path: "/burrow".into(),
            tls: None,
            keepalive: Duration::from_secs(25),
        });
    }
    settings
}

pub async fn start_server(settings: ServerSettings) -> RunningServer {
    let settings = as_websocket(settings);
    let settings_ws = settings.websocket.clone();
    let names: Vec<String> = settings.services.keys().cloned().collect();
    let server = Server::bind(settings).await.expect("server binds");
    let ws_addr = server.websocket_addr();
    let ws_settings = settings_ws.map(|mut w| {
        w.listen = ws_addr.expect("websocket bound");
        w
    });
    let addr = server
        .local_addr()
        .or(ws_addr)
        .expect("server has a listener");
    let services = names
        .iter()
        .map(|n| (n.clone(), server.service_addr(n).unwrap()))
        .collect();
    let shared = server.shared();
    let (reload_tx, reload_rx) = mpsc::channel(4);
    let (stop, stop_rx) = watch::channel(false);
    let task = tokio::spawn(server.run(reload_rx, stop_rx));
    RunningServer {
        addr,
        ws_addr,
        ws_settings,
        services,
        reload_tx,
        stop,
        task,
        shared,
    }
}

impl RunningServer {
    pub async fn reload(&self, mut next: ServerSettings) {
        if ws_mode() && next.websocket.is_none() {
            next.listen = None;
            next.websocket = self.ws_settings.clone();
        }
        self.reload_tx.send(next).await.unwrap();
    }
}

pub struct RunningClient {
    pub state: watch::Receiver<State>,
    pub stop: watch::Sender<bool>,
    pub task: JoinHandle<burrow::client::RunEnd>,
}

pub async fn start_client(mut settings: ClientSettings) -> RunningClient {
    if ws_mode()
        && let burrow::transport::endpoint::Endpoint::Tcp { host_port } = &settings.endpoint
    {
        let url = format!("ws://{host_port}/burrow");
        settings.endpoint = burrow::transport::endpoint::Endpoint::parse(&url).unwrap();
        settings.server = url;
    }
    start_client_with(Client::new(settings)).await
}

pub async fn start_client_with(client: Client) -> RunningClient {
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
    let l = burrow::transport::tcp::bind_tcp("127.0.0.1:0".parse().unwrap()).unwrap();
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
    logs();
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

pub fn logs() {
    if let Ok(filter) = std::env::var("BURROW_LOG") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_test_writer()
            .try_init();
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
