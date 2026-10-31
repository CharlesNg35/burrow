mod common;

use std::path::Path;

use burrow_web::{ClientAuth, ProxySettings, Site};
use common::devices::*;
use common::*;
use tokio_rustls::rustls::HandshakeKind;

fn protected(service: &str, hosts: &[&str], key: &ClientAuth) -> Site {
    let mut s = https_site(service, hosts);
    s.options.client_auth = Some(key.clone());
    s
}

struct World {
    proxy: burrow_web::Proxy<Fake>,
    sites: Vec<Site>,
}

impl World {
    fn addr(&self) -> std::net::SocketAddr {
        self.proxy.https_addr().unwrap()
    }

    async fn reload(&self) {
        let mut s = ProxySettings::new(self.addr());
        s.sites = self.sites.clone();
        self.proxy.apply(&s).await;
    }
}

async fn world(sites: Vec<Site>, names: &[&str]) -> World {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    for s in &sites {
        up.serve(&s.service, app.addr);
    }
    let kept = sites.clone();
    let proxy = start_with(|s| s.sites = kept, up, fixed(names), false).await;
    World { proxy, sites }
}

fn files(dir: &Path, ca: &DeviceCa, crl_bytes: Option<&[u8]>) -> ClientAuth {
    ClientAuth {
        ca: write(dir, "ca.pem", ca.pem.as_bytes()),
        crl: crl_bytes.map(|c| write(dir, "devices.crl", c)),
    }
}

fn ok(r: &Option<String>) -> bool {
    r.as_deref().is_some_and(|t| t.starts_with("HTTP/1.1 200"))
}

#[tokio::test]
async fn only_a_device_from_the_ca_gets_in_and_other_names_ask_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ca = device_ca("Devices CA", None);
    let other = device_ca("Other CA", None);
    let key = files(dir.path(), &ca, None);
    let w = world(
        vec![
            protected("grafana", &["grafana.example.com"], &key),
            https_site("blog", &["blog.example.com"]),
        ],
        &["*.example.com"],
    )
    .await;
    let g = "grafana.example.com";
    assert!(get(w.addr(), g, g, &client(None, None)).await.is_none());
    let stranger = device(&other, 5, &[]);
    assert!(
        get(w.addr(), g, g, &client(Some(&stranger), None))
            .await
            .is_none()
    );
    let mine = device(&ca, 6, &[]);
    assert!(ok(&get(w.addr(), g, g, &client(Some(&mine), None)).await));
    let b = "blog.example.com";
    assert!(ok(&get(w.addr(), b, b, &client(None, None)).await));
}

#[tokio::test]
async fn one_wildcard_over_an_open_and_a_protected_site_gives_421_across() {
    let dir = tempfile::tempdir().unwrap();
    let ca = device_ca("Devices CA", None);
    let key = files(dir.path(), &ca, None);
    let w = world(
        vec![
            https_site("apps", &["*.example.com"]),
            protected("secret", &["secret.example.com"], &key),
        ],
        &["*.example.com"],
    )
    .await;
    let open = "www.example.com";
    let secret = "secret.example.com";
    let r = get(w.addr(), open, secret, &client(None, None))
        .await
        .unwrap();
    assert!(r.starts_with("HTTP/1.1 421"), "{r}");
    let mine = device(&ca, 1, &[]);
    let r = get(w.addr(), secret, open, &client(Some(&mine), None))
        .await
        .unwrap();
    assert!(r.starts_with("HTTP/1.1 421"), "{r}");
    assert!(
        get(w.addr(), secret, secret, &client(None, None))
            .await
            .is_none()
    );
    assert!(ok(&get(
        w.addr(),
        secret,
        secret,
        &client(Some(&mine), None)
    )
    .await));
}

#[tokio::test]
async fn one_wildcard_over_an_open_and_a_protected_site_gives_421_across_http2() {
    let dir = tempfile::tempdir().unwrap();
    let ca = device_ca("Devices CA", None);
    let key = files(dir.path(), &ca, None);
    let w = world(
        vec![
            https_site("apps", &["*.example.com"]),
            protected("secret", &["secret.example.com"], &key),
        ],
        &["*.example.com"],
    )
    .await;
    let open = "www.example.com";
    let secret = "secret.example.com";
    let alpn = &[b"h2".as_slice(), b"http/1.1".as_slice()];
    let on_open = client_alpn(None, None, alpn);
    assert_eq!(
        get_h2(w.addr(), open, secret, &on_open).await,
        Some(421),
        "a request for the protected name on the open connection is misdirected"
    );
    let mine = device(&ca, 1, &[]);
    let on_secret = client_alpn(Some(&mine), None, alpn);
    assert_eq!(
        get_h2(w.addr(), secret, open, &on_secret).await,
        Some(421),
        "and the other way across the same wildcard"
    );
    assert_eq!(
        get_h2(w.addr(), secret, secret, &on_secret).await,
        Some(200),
        "the matching name with the device certificate is served"
    );
}

#[tokio::test]
async fn a_session_from_the_open_name_never_resumes_on_the_protected_one() {
    let dir = tempfile::tempdir().unwrap();
    let ca = device_ca("Devices CA", None);
    let key = files(dir.path(), &ca, None);
    let w = world(
        vec![
            https_site("apps", &["*.example.com"]),
            protected("secret", &["secret.example.com"], &key),
        ],
        &["*.example.com"],
    )
    .await;
    let store = name_blind();
    let c = client(None, Some(store.clone()));
    assert_eq!(
        kind(w.addr(), "www.example.com", &c).await,
        Some(HandshakeKind::Full)
    );
    assert_eq!(
        kind(w.addr(), "www.example.com", &c).await,
        Some(HandshakeKind::Resumed)
    );
    let _ = kind(w.addr(), "www.example.com", &c).await;
    assert_eq!(kind(w.addr(), "secret.example.com", &c).await, None);
    let mine = device(&ca, 1, &[]);
    let with_cert = client(Some(&mine), Some(store));
    let _ = kind(w.addr(), "www.example.com", &with_cert).await;
    assert_eq!(
        kind(w.addr(), "secret.example.com", &with_cert).await,
        Some(HandshakeKind::Full)
    );
}

#[tokio::test]
async fn an_ip_address_site_asks_for_the_device_certificate_too() {
    let dir = tempfile::tempdir().unwrap();
    let ca = device_ca("Devices CA", None);
    let key = files(dir.path(), &ca, None);
    let w = world(vec![protected("nas", &["127.0.0.1"], &key)], &["127.0.0.1"]).await;
    let ip = "127.0.0.1";
    assert!(get(w.addr(), ip, ip, &client(None, None)).await.is_none());
    let mine = device(&ca, 1, &[]);
    assert!(ok(&get(w.addr(), ip, ip, &client(Some(&mine), None)).await));
}

#[tokio::test]
async fn a_revoked_device_is_refused_and_a_new_crl_revokes_live_without_resumption() {
    let dir = tempfile::tempdir().unwrap();
    let ca = device_ca("Devices CA", None);
    let key = files(dir.path(), &ca, Some(&crl(&ca, &[2], next_week())));
    let w = world(
        vec![protected("g", &["g.example.com"], &key)],
        &["g.example.com"],
    )
    .await;
    let g = "g.example.com";
    let lost = device(&ca, 2, &[]);
    assert!(
        get(w.addr(), g, g, &client(Some(&lost), None))
            .await
            .is_none()
    );
    let phone = device(&ca, 3, &[]);
    let store = name_blind();
    let c = client(Some(&phone), Some(store));
    assert_eq!(kind(w.addr(), g, &c).await, Some(HandshakeKind::Full));
    assert_eq!(kind(w.addr(), g, &c).await, Some(HandshakeKind::Resumed));
    let _ = kind(w.addr(), g, &c).await;
    write(dir.path(), "devices.crl", &crl(&ca, &[2, 3], next_week()));
    w.reload().await;
    assert_eq!(
        kind(w.addr(), g, &c).await,
        None,
        "no resumption after the change"
    );
    assert!(
        get(w.addr(), g, g, &client(Some(&phone), None))
            .await
            .is_none()
    );
    let laptop = device(&ca, 4, &[]);
    assert!(ok(&get(w.addr(), g, g, &client(Some(&laptop), None)).await));
}

#[tokio::test]
async fn an_expired_crl_still_applies_and_shows_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let ca = device_ca("Devices CA", None);
    let past = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    let key = files(dir.path(), &ca, Some(&crl(&ca, &[2], past)));
    let w = world(
        vec![protected("g", &["g.example.com"], &key)],
        &["g.example.com"],
    )
    .await;
    let g = "g.example.com";
    assert!(
        get(w.addr(), g, g, &client(Some(&device(&ca, 2, &[])), None))
            .await
            .is_none()
    );
    assert!(ok(&get(
        w.addr(),
        g,
        g,
        &client(Some(&device(&ca, 3, &[])), None)
    )
    .await));
    let notes = w.proxy.access_notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].0, "g");
    assert!(notes[0].1.contains("expired on"), "{notes:?}");
}

#[tokio::test]
async fn a_two_tier_ca_with_only_the_intermediates_crl_refuses_only_the_listed_device() {
    let dir = tempfile::tempdir().unwrap();
    let root = device_ca("Root CA", None);
    let mid = device_ca("Intermediate CA", Some(&root));
    let both = format!("{}{}", root.pem, mid.pem);
    let key = ClientAuth {
        ca: write(dir.path(), "ca.pem", both.as_bytes()),
        crl: Some(write(
            dir.path(),
            "devices.crl",
            &crl(&mid, &[8], next_week()),
        )),
    };
    let w = world(
        vec![protected("g", &["g.example.com"], &key)],
        &["g.example.com"],
    )
    .await;
    let g = "g.example.com";
    let good = device(&mid, 7, &[&mid]);
    assert!(ok(&get(w.addr(), g, g, &client(Some(&good), None)).await));
    let revoked = device(&mid, 8, &[&mid]);
    assert!(
        get(w.addr(), g, g, &client(Some(&revoked), None))
            .await
            .is_none()
    );
    let two = [crl(&mid, &[8], next_week()), crl(&root, &[], next_week())].concat();
    write(dir.path(), "devices.crl", &two);
    w.reload().await;
    assert!(ok(&get(w.addr(), g, g, &client(Some(&good), None)).await));
    assert!(
        get(w.addr(), g, g, &client(Some(&revoked), None))
            .await
            .is_none()
    );
    assert!(w.proxy.access_notes().is_empty());
}

#[tokio::test]
async fn a_corrupt_replacement_keeps_the_last_good_crl_in_force() {
    let dir = tempfile::tempdir().unwrap();
    let ca = device_ca("Devices CA", None);
    let key = files(dir.path(), &ca, Some(&crl(&ca, &[2], next_week())));
    let w = world(
        vec![protected("g", &["g.example.com"], &key)],
        &["g.example.com"],
    )
    .await;
    let g = "g.example.com";
    write(dir.path(), "devices.crl", b"-----BEGIN X509 CRL-----\nAAAA");
    w.reload().await;
    assert!(
        get(w.addr(), g, g, &client(Some(&device(&ca, 2, &[])), None))
            .await
            .is_none()
    );
    assert!(ok(&get(
        w.addr(),
        g,
        g,
        &client(Some(&device(&ca, 3, &[])), None)
    )
    .await));
    let notes = w.proxy.access_notes();
    assert!(
        notes[0].1.contains("the last good files stay in force"),
        "{notes:?}"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_reload_reads_client_auth_files_off_the_async_workers() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let ca = device_ca("Devices CA", None);
    let fifo = dir.path().join("ca.pem");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let w = std::sync::Arc::new(
        world(
            vec![https_site("blog", &["blog.example.com"])],
            &["*.example.com"],
        )
        .await,
    );
    let key = ClientAuth {
        ca: fifo.clone(),
        crl: None,
    };
    let mut s = ProxySettings::new(w.addr());
    s.sites = w.sites.clone();
    s.sites
        .push(protected("grafana", &["grafana.example.com"], &key));
    let (opened_tx, opened_rx) = std::sync::mpsc::channel();
    let (probe_tx, probe_rx) = std::sync::mpsc::channel::<()>();
    let pem = ca.pem.clone();
    let writer = std::thread::spawn(move || {
        let mut f = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        opened_tx.send(()).unwrap();
        let ran = probe_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_ok();
        f.write_all(pem.as_bytes()).unwrap();
        ran
    });
    let applying = {
        let w = w.clone();
        tokio::spawn(async move { w.proxy.apply(&s).await })
    };
    opened_rx.recv().unwrap();
    tokio::spawn(async move { probe_tx.send(()) });
    applying.await.unwrap();
    assert!(
        writer.join().unwrap(),
        "a task ran while the file was being read"
    );
}

async fn on_port_80(sites: Vec<Site>, trusted: bool) -> (burrow_web::Proxy<Fake>, HttpBackend) {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    for s in &sites {
        up.serve(&s.service, app.addr);
    }
    let proxy = start_with(
        |s| {
            s.sites = sites;
            if trusted {
                s.forwarding.trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
            }
        },
        up,
        fixed(&["*.example.com"]),
        true,
    )
    .await;
    (proxy, app)
}

fn plain_get(host: &str, extra: &str) -> String {
    format!("GET /x HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n")
}

#[tokio::test]
async fn a_protected_site_is_never_served_on_port_80_even_with_https_redirect_false() {
    let dir = tempfile::tempdir().unwrap();
    let key = files(dir.path(), &device_ca("Devices CA", None), None);
    let (proxy, app) = on_port_80(
        vec![
            Site {
                https_redirect: false,
                ..protected("nas", &["nas.example.com"], &key)
            },
            Site {
                https_redirect: false,
                ..https_site("blog", &["blog.example.com"])
            },
        ],
        false,
    )
    .await;
    let http = proxy.http_addr().unwrap();
    let https = proxy.https_addr().unwrap().port();
    let r = over_plain(http, &plain_get("nas.example.com", "")).await;
    assert!(r.starts_with("HTTP/1.1 308"), "{r}");
    let want = format!("location: https://nas.example.com:{https}/x\r\n");
    assert!(r.to_ascii_lowercase().contains(&want), "{r}");
    assert_eq!(app.connections.load(std::sync::atomic::Ordering::SeqCst), 0);
    let r = over_plain(http, &plain_get("blog.example.com", "")).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
}

#[tokio::test]
async fn a_trusted_proxys_forwarded_https_does_not_open_a_protected_site_on_port_80() {
    let dir = tempfile::tempdir().unwrap();
    let key = files(dir.path(), &device_ca("Devices CA", None), None);
    let (proxy, app) = on_port_80(
        vec![
            protected("nas", &["*.nas.example.com"], &key),
            https_site("blog", &["blog.example.com"]),
        ],
        true,
    )
    .await;
    let http = proxy.http_addr().unwrap();
    let proto = "X-Forwarded-Proto: https\r\n";
    for host in ["a.nas.example.com", "A.NAS.example.com."] {
        let r = over_plain(http, &plain_get(host, proto)).await;
        assert!(r.starts_with("HTTP/1.1 308"), "{host}: {r}");
    }
    assert_eq!(app.connections.load(std::sync::atomic::Ordering::SeqCst), 0);
    let r = over_plain(http, &plain_get("blog.example.com", proto)).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
}
