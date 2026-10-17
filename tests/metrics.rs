mod common;

use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use burrow::allowlist::Sources;
use burrow::config::model::Protocol;
use burrow::config::settings::{Health, Service};
use burrow::metrics::Status;
use burrow::metrics::query::fetch;
use burrow::secret::Secret;
use common::*;

fn http_get(addr: SocketAddr, path: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
        .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

async fn status(addr: SocketAddr) -> Status {
    tokio::task::spawn_blocking(move || fetch(addr).unwrap())
        .await
        .unwrap()
}

async fn metered(services: Vec<(&str, Service)>) -> Tunnel {
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings.metrics = Some("127.0.0.1:0".parse().unwrap());
    tunnel_with(settings, services, |c| c, secret).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_are_off_unless_configured() {
    let echo = echo_server().await;
    let t = tunnel(vec![("web", service(echo, Protocol::Tcp))]).await;
    assert_eq!(t.server.metrics, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_counts_connections_and_bytes() {
    let echo = echo_server().await;
    let t = metered(vec![("web", service(echo, Protocol::Tcp))]).await;
    let addr = t.server.metrics.unwrap();
    for seed in 0..3 {
        let data = pattern(10_000, seed);
        assert_eq!(roundtrip(t.server.services["web"], &data).await, data);
    }
    let mut s = status(addr).await;
    for _ in 0..50 {
        if s.services[0].open == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        s = status(addr).await;
    }
    assert_eq!(s.version, env!("CARGO_PKG_VERSION"));
    let home = &s.clients[0];
    assert_eq!((home.name.as_str(), home.connected), ("home", true));
    assert!(
        home.from
            .as_deref()
            .is_some_and(|f| f.starts_with("127.0.0.1:"))
    );
    let web = &s.services[0];
    assert_eq!(web.name, "web");
    assert_eq!((web.total, web.open, web.refused), (3, 0, 0));
    assert_eq!((web.bytes_in, web.bytes_out), (30_000, 30_000));
    let text = http_get(addr, "/metrics");
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
    for line in [
        "burrow_service_connections_total{service=\"web\"} 3",
        "burrow_service_bytes_total{service=\"web\",direction=\"in\"} 30000",
        "burrow_client_connected{client=\"home\"} 1",
    ] {
        assert!(text.lines().any(|l| l == line), "missing {line} in\n{text}");
    }
    assert!(http_get(addr, "/nope").starts_with("HTTP/1.1 404"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refusals_and_health_show_up() {
    let echo = echo_server().await;
    let blocked = Service {
        sources: Sources {
            allow: vec![],
            deny: vec!["127.0.0.0/8".parse().unwrap()],
        },
        ..service(echo, Protocol::Tcp)
    };
    let checked = Service {
        health: Some(Health {
            interval: Duration::from_millis(200),
            timeout: Duration::from_secs(1),
            failures: 1,
        }),
        ..service(echo, Protocol::Tcp)
    };
    let t = metered(vec![("blocked", blocked), ("checked", checked)]).await;
    let _ = try_roundtrip(t.server.services["blocked"], b"x").await;
    let addr = t.server.metrics.unwrap();
    let mut s = status(addr).await;
    for _ in 0..50 {
        if !s.services[1].health.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        s = status(addr).await;
    }
    assert_eq!(s.services[0].name, "blocked");
    assert_eq!(s.services[0].refused, 1);
    assert_eq!(s.services[1].health.len(), 1);
    assert!(s.services[1].health[0].up);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_turns_the_endpoint_on_and_off() {
    let secret = Secret::generate();
    let settings = server_settings(vec![secret.clone()]);
    let t = tunnel_with(settings.clone(), vec![], |c| c, secret).await;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let mut on = settings.clone();
    on.metrics = Some(port);
    t.server.reload(on).await;
    let mut up = false;
    for _ in 0..50 {
        if std::net::TcpStream::connect(port).is_ok() {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(up, "metrics must start after reload");
    assert_eq!(status(port).await.clients[0].name, "home");
    t.server.reload(settings).await;
    for _ in 0..50 {
        if std::net::TcpStream::connect(port).is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("metrics must stop after reload");
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn burrow_status_prints_the_running_server() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("server.yaml");
    let (listen, metrics) = (free_port(), free_port());
    std::fs::write(
        &config,
        format!(
            "listen: 127.0.0.1:{listen}\nclients:\n  home:\n    secret: {}\nservices:\n  web:\n    client: home\n    bind: 127.0.0.1:{}\n    target: 3000\nmetrics:\n  listen: {metrics}\n",
            Secret::generate().encode(),
            free_port()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let bin = env!("CARGO_BIN_EXE_burrow");
    let _server = Running(
        Command::new(bin)
            .args(["server", "-c"])
            .arg(&config)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut out = None;
    for _ in 0..50 {
        let o = Command::new(bin)
            .args(["status", "-c"])
            .arg(&config)
            .output()
            .unwrap();
        if o.status.success() {
            out = Some(String::from_utf8(o.stdout).unwrap());
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let text = out.expect("burrow status succeeds once the server runs");
    assert!(
        text.starts_with(&format!("burrow {}, up ", env!("CARGO_PKG_VERSION"))),
        "{text}"
    );
    assert!(
        text.contains("home  offline, not seen since the server started"),
        "{text}"
    );
    assert!(text.contains("web  tcp 127.0.0.1:"), "{text}");
    let json = Command::new(bin)
        .args(["status", "--json", "-c"])
        .arg(&config)
        .output()
        .unwrap();
    let parsed: Status = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(parsed.services[0].name, "web");

    let off = dir.path().join("off.yaml");
    std::fs::write(
        &off,
        std::fs::read_to_string(&config)
            .unwrap()
            .split("metrics:")
            .next()
            .unwrap(),
    )
    .unwrap();
    let o = Command::new(bin)
        .args(["status", "-c"])
        .arg(&off)
        .output()
        .unwrap();
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("does not turn on"), "{err}");
    assert!(err.contains("listen: 127.0.0.1:9100"), "{err}");
}
