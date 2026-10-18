#![allow(clippy::disallowed_macros)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn arg(args: &[String], name: &str, default: &str) -> String {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

fn addr(args: &[String]) -> SocketAddr {
    arg(args, "--addr", "127.0.0.1:9000")
        .parse()
        .expect("--addr host:port")
}

async fn echo(args: &[String]) {
    let listen: SocketAddr = arg(args, "--listen", "127.0.0.1:9000").parse().unwrap();
    let l = burrow::transport::tcp::bind_tcp(listen).unwrap();
    println!("echo listening on {listen}");
    loop {
        let (mut s, _) = l.accept().await.unwrap();
        let _ = s.set_nodelay(true);
        tokio::spawn(async move {
            let (mut r, mut w) = s.split();
            let _ = tokio::io::copy(&mut r, &mut w).await;
        });
    }
}

async fn latency(args: &[String]) {
    let count: usize = arg(args, "--count", "20000").parse().unwrap();
    let size: usize = arg(args, "--size", "64").parse().unwrap();
    let fail = |e: std::io::Error| -> ! {
        eprintln!("latency: {e}");
        std::process::exit(1)
    };
    let mut s = TcpStream::connect(addr(args))
        .await
        .unwrap_or_else(|e| fail(e));
    s.set_nodelay(true).unwrap();
    let msg = vec![7u8; size];
    let mut buf = vec![0u8; size];
    let warmup = count.min(200);
    for _ in 0..warmup {
        s.write_all(&msg).await.unwrap_or_else(|e| fail(e));
        s.read_exact(&mut buf).await.unwrap_or_else(|e| fail(e));
    }
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        let t = Instant::now();
        s.write_all(&msg).await.unwrap();
        s.read_exact(&mut buf).await.unwrap();
        samples.push(t.elapsed());
    }
    samples.sort();
    let p = |q: f64| {
        samples[((samples.len() as f64 * q) as usize).min(samples.len() - 1)].as_secs_f64() * 1e6
    };
    println!(
        "{{\"p50_us\": {:.1}, \"p99_us\": {:.1}, \"p999_us\": {:.1}, \"count\": {count}, \"size\": {size}}}",
        p(0.5),
        p(0.99),
        p(0.999)
    );
}

async fn connrate(args: &[String]) {
    let secs: u64 = arg(args, "--secs", "10").parse().unwrap();
    let conc: usize = arg(args, "--concurrency", "32").parse().unwrap();
    let target = addr(args);
    let done = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let mut tasks = Vec::new();
    for _ in 0..conc {
        let (done, failed, stop) = (done.clone(), failed.clone(), stop.clone());
        tasks.push(tokio::spawn(async move {
            let mut buf = [0u8; 16];
            while !stop.load(Ordering::Relaxed) {
                let ok = async {
                    let mut s =
                        tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(target))
                            .await
                            .ok()?
                            .ok()?;
                    s.set_nodelay(true).ok()?;
                    s.write_all(b"0123456789abcdef").await.ok()?;
                    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut buf))
                        .await
                        .ok()?
                        .ok()?;
                    Some(())
                }
                .await;
                if ok.is_some() {
                    done.fetch_add(1, Ordering::Relaxed);
                } else {
                    failed.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    tokio::time::sleep(Duration::from_secs(secs)).await;
    stop.store(true, Ordering::Relaxed);
    for t in tasks {
        let _ = t.await;
    }
    println!(
        "{{\"conn_per_sec\": {:.0}, \"failed\": {}, \"secs\": {secs}, \"concurrency\": {conc}}}",
        done.load(Ordering::Relaxed) as f64 / secs as f64,
        failed.load(Ordering::Relaxed)
    );
}

async fn idle(args: &[String]) {
    let n: usize = arg(args, "--count", "1000").parse().unwrap();
    let hold: u64 = arg(args, "--hold-secs", "3600").parse().unwrap();
    let target = addr(args);
    let mut conns = Vec::with_capacity(n);
    let gate = Arc::new(tokio::sync::Semaphore::new(256));
    let mut tasks = Vec::with_capacity(n);
    for _ in 0..n {
        let gate = gate.clone();
        tasks.push(tokio::spawn(async move {
            let _p = gate.acquire_owned().await.unwrap();
            let mut s = TcpStream::connect(target).await.ok()?;
            s.write_all(b"ping").await.ok()?;
            let mut b = [0u8; 4];
            tokio::time::timeout(Duration::from_secs(30), s.read_exact(&mut b))
                .await
                .ok()?
                .ok()?;
            Some(s)
        }));
    }
    let mut failed = 0;
    for t in tasks {
        match t.await.unwrap() {
            Some(s) => conns.push(s),
            None => failed += 1,
        }
    }
    println!("{{\"open\": {}, \"failed\": {failed}}}", conns.len());
    tokio::time::sleep(Duration::from_secs(hold)).await;
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("echo") => echo(&args).await,
        Some("latency") => latency(&args).await,
        Some("connrate") => connrate(&args).await,
        Some("idle") => idle(&args).await,
        _ => {
            eprintln!(
                "usage: loadgen echo|latency|connrate|idle [--addr host:port] [--listen host:port] [--count n] [--secs n] [--concurrency n]"
            );
            std::process::exit(2);
        }
    }
}
