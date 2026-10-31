use std::net::SocketAddr;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep};

use crate::kit::*;

const CHILD: &str = "BURROW_H2_LOAD_APP";
const CHILD_H2C: &str = "BURROW_H2_LOAD_H2C";
const READY: &str = "burrow-h2-load-port=";
const STREAMS: usize = burrow_web::http::h2::MAX_STREAMS as usize;
const GETS: usize = 48;
const SLOW: usize = 5;
const UPLOADS: usize = STREAMS - GETS - SLOW;
const MIB: usize = 1024 * 1024;
const SLOW_BODY: usize = 512 * 1024;
const SLOW_RATE: f64 = 64.0 * 1024.0;
const RUN: Duration = Duration::from_secs(12);
const CAP_KIB: u64 = 8 * 1024;
const BOUND_KIB: u64 = 2 * CAP_KIB;
const BASE_CONNS: usize = 10;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_as_the_measured_process() {
    let Some(app) = std::env::var_os(CHILD) else {
        return;
    };
    let addr: SocketAddr = app.to_str().unwrap().parse().unwrap();
    let app = App {
        addr,
        counts: Arc::default(),
    };
    let h2c = std::env::var_os(CHILD_H2C).is_some();
    let p = proxy(&app, |s| {
        s.max_pending = 4096;
        for site in &mut s.sites {
            site.options.target_http2 = h2c;
        }
    })
    .await;
    println!("\n{READY}{}", p.https_addr().unwrap().port());
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    let wait = tokio::task::spawn_blocking(|| {
        let mut rest = Vec::new();
        let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut rest);
    });
    let _ = wait.await;
}

struct Server {
    child: Child,
    pid: u32,
    addr: SocketAddr,
}

async fn spawn_server(app: SocketAddr, h2c: bool) -> Server {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    if h2c {
        cmd.env(CHILD_H2C, "1");
    }
    let mut child = cmd
        .args([
            "--exact",
            "load::serve_as_the_measured_process",
            "--nocapture",
        ])
        .env(CHILD, app.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let port = loop {
        let line = lines.next_line().await.unwrap().expect("the server's port");
        if let Some(p) = line.strip_prefix(READY) {
            break p.parse::<u16>().unwrap();
        }
    };
    Server {
        child,
        pid,
        addr: SocketAddr::from(([127, 0, 0, 1], port)),
    }
}

fn status_kib(pid: u32, key: &str) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix(key))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .unwrap()
}

#[derive(Default)]
struct Tally {
    gets: AtomicUsize,
    uploads: AtomicUsize,
    slow: AtomicUsize,
    peak_kib: AtomicU64,
}

async fn slow_read(c: &Client) {
    let mut c = c.clone().ready().await.unwrap();
    let uri = site_uri(&format!("/big/{SLOW_BODY}"));
    let (resp, _) = c.send_request(request(&uri, &[]), true).unwrap();
    let resp = resp.await.unwrap();
    assert_eq!(resp.status(), 200);
    let mut body = resp.into_body();
    let mut got = 0;
    while let Some(chunk) = body.data().await {
        let chunk = chunk.unwrap();
        sleep(Duration::from_secs_f64(chunk.len() as f64 / SLOW_RATE)).await;
        let _ = body.flow_control().release_capacity(chunk.len());
        got += chunk.len();
    }
    assert_eq!(got, SLOW_BODY);
}

fn answer_within(conns: usize) -> Duration {
    let load = u32::try_from(conns.div_ceil(BASE_CONNS)).unwrap();
    ANSWER * load.max(1)
}

async fn one_stream(
    mut c: Client,
    slot: usize,
    until: Instant,
    within: Duration,
    tally: Arc<Tally>,
) {
    let upload = Bytes::from(vec![b'u'; MIB]);
    while Instant::now() < until {
        match slot {
            s if s < GETS => {
                let get = request(&site_uri(&format!("/big/{MIB}")), &[]);
                let (status, body) = send_within(&mut c, get, None, within).await;
                assert_eq!((status, body.len()), (200, MIB));
                tally.gets.fetch_add(1, Ordering::Relaxed);
            }
            s if s < GETS + UPLOADS => {
                let mut post = request(&site_uri("/up"), &[]);
                *post.method_mut() = http::Method::POST;
                let (status, body) = send_within(&mut c, post, Some(upload.clone()), within).await;
                assert_eq!(status, 200);
                assert!(body.contains(&format!("body-len: {MIB}\n")), "{body}");
                tally.uploads.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                slow_read(&c).await;
                tally.slow.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

async fn browser_like(addr: SocketAddr) -> Client {
    let mut b = h2::client::Builder::new();
    b.initial_connection_window_size(15 * u32::try_from(MIB).unwrap());
    client_with(addr, &b).await
}

async fn measure(conns: usize, h2c: bool) -> (u64, u64, u64, Arc<Tally>) {
    let a = if h2c { app_h2c().await } else { app().await };
    let mut server = spawn_server(a.addr, h2c).await;
    sleep(Duration::from_millis(500)).await;
    let idle = status_kib(server.pid, "VmRSS:");
    let tally = Arc::new(Tally::default());
    let mut clients = Vec::new();
    for _ in 0..conns {
        clients.push(browser_like(server.addr).await);
    }
    let until = Instant::now() + RUN;
    let mut tasks = Vec::new();
    for c in &clients {
        for slot in 0..STREAMS {
            tasks.push(tokio::spawn(one_stream(
                c.clone(),
                slot,
                until,
                answer_within(conns),
                tally.clone(),
            )));
        }
    }
    let pid = server.pid;
    let t = tally.clone();
    let sampler = tokio::spawn(async move {
        loop {
            t.peak_kib
                .fetch_max(status_kib(pid, "VmRSS:"), Ordering::Relaxed);
            sleep(Duration::from_millis(100)).await;
        }
    });
    for task in tasks {
        task.await.unwrap();
    }
    sampler.abort();
    let hwm = status_kib(server.pid, "VmHWM:");
    drop(server.child.stdin.take());
    let _ = server.child.wait().await;
    (idle, tally.peak_kib.load(Ordering::Relaxed), hwm, tally)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_memory_per_connection_under_legitimate_load_stays_bounded() {
    if std::env::var_os("BURROW_H2_SLOW").is_none() {
        return;
    }
    let conns = std::env::var("BURROW_H2_LOAD_CONNS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(BASE_CONNS);
    for (h2c, app) in [(false, "HTTP/1.1 app"), (true, "h2c app")] {
        let (idle, peak, hwm, tally) = measure(conns, h2c).await;
        let per_conn = hwm.saturating_sub(idle) / u64::try_from(conns).unwrap();
        println!(
            "h2 load: {app} N={conns} streams/conn={STREAMS} idle={idle} KiB peak-rss={peak} KiB \
             hwm={hwm} KiB per-conn={per_conn} KiB cap={CAP_KIB} KiB answer-within={:?} gets={} uploads={} slow={}",
            answer_within(conns),
            tally.gets.load(Ordering::Relaxed),
            tally.uploads.load(Ordering::Relaxed),
            tally.slow.load(Ordering::Relaxed),
        );
        assert!(tally.slow.load(Ordering::Relaxed) >= conns * SLOW);
        assert!(
            per_conn <= BOUND_KIB,
            "{app}: {per_conn} KiB per connection, bound {BOUND_KIB} KiB"
        );
    }
}
