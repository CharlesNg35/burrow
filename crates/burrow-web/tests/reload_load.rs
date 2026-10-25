mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use burrow_web::{ProxySettings, Site, SiteOptions};
use common::*;

const SITES: usize = 40;

fn sites(round: usize) -> Vec<Site> {
    let mut out: Vec<Site> = (0..SITES)
        .map(|i| https_site(&format!("s{i}"), &[&format!("s{i}.example.com")]))
        .collect();
    out.push(Site {
        options: SiteOptions {
            paths: vec![format!("/round{}", round % 3)],
            ..SiteOptions::default()
        },
        ..https_site("moving", &["s0.example.com"])
    });
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn many_sites_keep_answering_while_the_config_reloads() {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    for i in 0..SITES {
        up.serve(&format!("s{i}"), app.addr);
    }
    up.serve("moving", app.addr);
    let proxy = Arc::new(start(|s| s.sites = sites(0), up).await);
    let addr = proxy.https_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (ok, bad) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let mut askers = Vec::new();
    for w in 0..4 {
        let (stop, ok, bad) = (stop.clone(), ok.clone(), bad.clone());
        askers.push(tokio::spawn(async move {
            let mut n = w;
            while !stop.load(Ordering::Relaxed) {
                let host = format!("s{}.example.com", n % SITES);
                let r = over_tls(
                    addr,
                    &host,
                    &format!("GET /x HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
                )
                .await;
                if r.starts_with("HTTP/1.1 200") {
                    ok.fetch_add(1, Ordering::Relaxed);
                } else {
                    bad.fetch_add(1, Ordering::Relaxed);
                }
                n += 4;
            }
        }));
    }
    for round in 1..=30 {
        let mut next = ProxySettings::new(addr);
        next.sites = sites(round);
        proxy.apply(&next).await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    stop.store(true, Ordering::Relaxed);
    for a in askers {
        a.await.unwrap();
    }
    assert_eq!(bad.load(Ordering::Relaxed), 0);
    assert!(
        ok.load(Ordering::Relaxed) > 40,
        "{}",
        ok.load(Ordering::Relaxed)
    );
    let t = proxy.table();
    let moving = t
        .route("s0.example.com", "/round0/a")
        .map(|r| r.prefix.clone());
    assert_eq!(moving.as_deref(), Some("/round0"));
}
