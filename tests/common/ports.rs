use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

const PORTS: std::ops::Range<u16> = 20_000..32_000;
const CLAIM_KEPT: Duration = Duration::from_secs(3600);

fn claim(port: u16) -> bool {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("test-ports");
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join(port.to_string());
    let stale = std::fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > CLAIM_KEPT);
    if stale {
        let _ = std::fs::remove_file(&file);
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .is_ok()
}

pub fn free_range(count: u16) -> u16 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let seed = std::process::id().wrapping_mul(2_654_435_761);
    let span = u32::from(PORTS.end - PORTS.start - count);
    for _ in 0..4000 {
        let n = NEXT.fetch_add(u32::from(count) + 1, Ordering::Relaxed);
        let base = PORTS.start + (seed.wrapping_add(n) % span) as u16;
        let all_free = (0..count).all(|i| {
            [[127, 0, 0, 1], [0, 0, 0, 0]].into_iter().all(|ip| {
                let addr = SocketAddr::from((ip, base + i));
                std::net::TcpListener::bind(addr).is_ok() && std::net::UdpSocket::bind(addr).is_ok()
            })
        });
        if all_free && (0..count).all(|i| claim(base + i)) {
            return base;
        }
    }
    panic!("no free range of {count} ports below the ephemeral range");
}

pub fn free_port() -> u16 {
    free_range(1)
}
