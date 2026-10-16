mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use burrow::client::State;
use burrow::config::model::Protocol;
use burrow::secret::Secret;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

async fn scaled_tunnel(services: Vec<(&str, burrow::config::settings::Service)>) -> Tunnel {
    let secret = Secret::generate();
    let mut s = server_settings(vec![secret.clone()]);
    s.max_connections_per_client = 1_000_000;
    s.max_pending = 100_000;
    s.max_handshakes = 4096;
    s.pool_max_idle = 256;
    s.handshake_timeout = Duration::from_secs(10);
    tunnel_with(s, services, |c| c, secret).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored"]
async fn ten_thousand_concurrent_connections() {
    let n = env_u64("BURROW_HEAVY_CONNS", 10_000) as usize;
    let echo = echo_server().await;
    let t = scaled_tunnel(vec![("web", service(echo, Protocol::Tcp))]).await;
    let addr = t.server.services["web"];
    let rss0 = rss_kib();
    let start = Instant::now();

    let opened = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let go = Arc::new(AtomicBool::new(false));
    let mut tasks = Vec::with_capacity(n);
    let gate = Arc::new(tokio::sync::Semaphore::new(512));
    for i in 0..n {
        let opened = opened.clone();
        let release = release.clone();
        let go = go.clone();
        let gate = gate.clone();
        tasks.push(tokio::spawn(async move {
            let permit = gate.acquire_owned().await.unwrap();
            let mut s = TcpStream::connect(addr).await.expect("connect");
            let hello = format!("conn {i} hello");
            s.write_all(hello.as_bytes()).await.unwrap();
            let mut buf = vec![0u8; hello.len()];
            tokio::time::timeout(Duration::from_secs(60), s.read_exact(&mut buf))
                .await
                .expect("first echo in time")
                .expect("first echo");
            assert_eq!(buf, hello.as_bytes());
            drop(permit);
            opened.fetch_add(1, Ordering::AcqRel);
            if !go.load(Ordering::Acquire) {
                release.notified().await;
            }
            let bye = format!("conn {i} bye");
            s.write_all(bye.as_bytes()).await.unwrap();
            s.shutdown().await.unwrap();
            let mut rest = Vec::new();
            s.read_to_end(&mut rest).await.unwrap();
            assert_eq!(rest, bye.as_bytes());
        }));
    }
    while opened.load(Ordering::Acquire) < n {
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            start.elapsed() < Duration::from_secs(180),
            "only {} of {n} opened",
            opened.load(Ordering::Acquire)
        );
    }
    let open_time = start.elapsed();
    let rss_open = rss_kib();
    assert_eq!(t.server.shared.active_connections(), n);
    go.store(true, Ordering::Release);
    release.notify_waiters();
    for task in tasks {
        task.await.unwrap();
    }
    let total = start.elapsed();
    eprintln!(
        "{n} concurrent connections: all open after {open_time:?}, done after {total:?}; process RSS {} MiB idle tunnel -> {} MiB with {n} open (test process holds both ends)",
        rss0 / 1024,
        rss_open / 1024
    );
    let shared = t.server.shared.clone();
    wait_for(|| shared.active_connections() == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored"]
async fn sustained_soak_without_leaks() {
    let secs = env_u64("BURROW_SOAK_SECS", 60);
    let workers = env_u64("BURROW_SOAK_WORKERS", 32) as usize;
    let echo = echo_server().await;
    let t = scaled_tunnel(vec![("web", service(echo, Protocol::Tcp))]).await;
    let addr = t.server.services["web"];
    let bytes = Arc::new(AtomicU64::new(0));
    let conns = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let mut samples = Vec::new();

    let mut tasks = Vec::new();
    for w in 0..workers {
        let (bytes, conns, stop) = (bytes.clone(), conns.clone(), stop.clone());
        tasks.push(tokio::spawn(async move {
            let mut round = 0u64;
            while !stop.load(Ordering::Relaxed) {
                round += 1;
                let size = if round.is_multiple_of(10) {
                    4 << 20
                } else {
                    1 + ((w as u64 * 7919 + round * 104729) % 65536) as usize
                };
                let data = pattern(size, round ^ w as u64);
                let got = roundtrip(addr, &data).await;
                assert!(got == data, "corrupted payload in soak");
                bytes.fetch_add(2 * size as u64, Ordering::Relaxed);
                conns.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }
    let start = Instant::now();
    let mut warm = 0;
    while start.elapsed() < Duration::from_secs(secs) {
        tokio::time::sleep(Duration::from_secs(5).min(Duration::from_secs(secs))).await;
        let rss = rss_kib();
        if warm == 0 && start.elapsed() > Duration::from_secs(secs / 4) {
            warm = rss;
        }
        samples.push(rss);
    }
    stop.store(true, Ordering::Relaxed);
    for task in tasks {
        task.await.unwrap();
    }
    let elapsed = start.elapsed().as_secs_f64();
    let shared = t.server.shared.clone();
    wait_for(|| shared.active_connections() == 0).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let end = rss_kib();
    let peak = samples.iter().copied().max().unwrap_or(0);
    eprintln!(
        "soak {elapsed:.0}s: {} connections, {:.1} GiB moved ({:.0} MiB/s through the tunnel), RSS after warm-up {} MiB, peak {} MiB, end {} MiB",
        conns.load(Ordering::Relaxed),
        bytes.load(Ordering::Relaxed) as f64 / (1u64 << 30) as f64,
        bytes.load(Ordering::Relaxed) as f64 / (1u64 << 20) as f64 / elapsed,
        warm / 1024,
        peak / 1024,
        end / 1024
    );
    let warm = if warm == 0 {
        samples.first().copied().unwrap_or(end)
    } else {
        warm
    };
    assert!(
        end < warm + 64 * 1024,
        "RSS grew from {warm} KiB to {end} KiB: possible leak"
    );
}

async fn chaos_proxy(upstream: SocketAddr, kill_every: Duration) -> (SocketAddr, Arc<AtomicU64>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let kills = Arc::new(AtomicU64::new(0));
    let k = kills.clone();
    tokio::spawn(async move {
        let mut seed = 0x9e3779b97f4a7c15u64;
        loop {
            let Ok((mut down, _)) = l.accept().await else {
                return;
            };
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let life = Duration::from_millis(seed % (kill_every.as_millis() as u64 * 2).max(1));
            let k = k.clone();
            tokio::spawn(async move {
                let Ok(mut up) = TcpStream::connect(upstream).await else {
                    return;
                };
                let copy = tokio::io::copy_bidirectional(&mut down, &mut up);
                if tokio::time::timeout(life, copy).await.is_err() {
                    k.fetch_add(1, Ordering::Relaxed);
                    let _ = socket2::SockRef::from(&down).set_linger(Some(Duration::ZERO));
                }
            });
        }
    });
    (addr, kills)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored"]
async fn chaos_dropped_connections_and_restarts() {
    let secs = env_u64("BURROW_CHAOS_SECS", 30);
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    settings.heartbeat_interval = Duration::from_millis(1000);
    let mut srv = start_server(settings.clone()).await;
    let svc_addr = srv.services["web"];
    let listen = srv.addr;
    let (proxy, kills) = chaos_proxy(listen, Duration::from_secs(2)).await;
    let mut c = client_settings(proxy, secret);
    c.reconnect_max = Duration::from_millis(300);
    let mut cli = start_client(c).await;
    wait_state(&mut cli.state, State::Connected).await;

    let start = Instant::now();
    let mut ok = 0u64;
    let mut failed = 0u64;
    let mut restarts = 0;
    let mut last_restart = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        let data = pattern(200_000, ok + failed);
        match try_roundtrip(svc_addr, &data).await {
            Some(got) if got == data => ok += 1,
            Some(got) if !got.is_empty() && !data.starts_with(&got) => {
                panic!("corrupted data under chaos")
            }
            _ => {
                failed += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        if last_restart.elapsed() > Duration::from_secs(3) {
            srv.stop.send(true).unwrap();
            srv.task.await.unwrap();
            settings.listen = listen;
            settings.services.get_mut("web").unwrap().bind = svc_addr;
            srv = start_server(settings.clone()).await;
            restarts += 1;
            last_restart = Instant::now();
        }
    }
    eprintln!(
        "chaos {secs}s: {ok} intact transfers, {failed} cut by chaos, {} proxy kills, {restarts} server restarts, 0 corrupted",
        kills.load(Ordering::Relaxed)
    );
    assert!(ok > failed, "most transfers should survive");
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    let data = pattern(1 << 20, 42);
    let mut recovered = false;
    for _ in 0..20 {
        if try_roundtrip(svc_addr, &data).await.as_deref() == Some(&data[..]) {
            recovered = true;
            break;
        }
    }
    assert!(recovered, "tunnel recovers after chaos");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: run with --ignored"]
async fn reconnect_cycles_do_not_leak() {
    let cycles = env_u64("BURROW_CYCLES", 300);
    let echo = echo_server().await;
    let secret = Secret::generate();
    let mut settings = server_settings(vec![secret.clone()]);
    settings
        .services
        .insert("web".into(), service(echo, Protocol::Tcp));
    let srv = start_server(settings).await;
    let mut rss_warm = 0;
    for i in 0..cycles {
        let mut cli = start_client(client_settings(srv.addr, secret.clone())).await;
        wait_state(&mut cli.state, State::Connected).await;
        let shared = srv.shared.clone();
        wait_for(|| shared.idle_channels("home") >= 1).await;
        assert_eq!(roundtrip(srv.services["web"], b"cycle").await, b"cycle");
        cli.stop.send(true).unwrap();
        cli.task.await.unwrap();
        let shared = srv.shared.clone();
        wait_for(|| shared.session_names().is_empty()).await;
        if i == cycles / 5 {
            rss_warm = rss_kib();
        }
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let end = rss_kib();
    eprintln!(
        "{cycles} client connect/disconnect cycles: RSS {} KiB after warm-up, {} KiB at end",
        rss_warm, end
    );
    assert!(
        end < rss_warm + 16 * 1024,
        "RSS grew from {rss_warm} KiB to {end} KiB over reconnect cycles"
    );
}
