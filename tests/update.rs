use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

const MINE: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
enum Reply {
    Body(Vec<u8>),
    Chunked(Vec<u8>),
    Redirect(String),
}

struct Releases {
    base: String,
    routes: Arc<Mutex<HashMap<String, Reply>>>,
}

impl Releases {
    fn start() -> Releases {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/releases", listener.local_addr().unwrap());
        let routes: Arc<Mutex<HashMap<String, Reply>>> = Arc::default();
        let shared = routes.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let mut req = Vec::new();
                let mut byte = [0u8; 1];
                while !req.ends_with(b"\r\n\r\n") && conn.read(&mut byte).unwrap_or(0) == 1 {
                    req.push(byte[0]);
                }
                let text = String::from_utf8_lossy(&req);
                let path = text.split_whitespace().nth(1).unwrap_or("").to_string();
                let reply = shared.lock().unwrap().get(&path).cloned();
                let head = |status: &str, extra: &str| {
                    format!("HTTP/1.1 {status}\r\n{extra}Connection: close\r\n\r\n")
                };
                let _ = match reply {
                    Some(Reply::Body(b)) => conn
                        .write_all(
                            head("200 OK", &format!("Content-Length: {}\r\n", b.len())).as_bytes(),
                        )
                        .and_then(|_| conn.write_all(&b)),
                    Some(Reply::Chunked(b)) => {
                        let mut out = head("200 OK", "Transfer-Encoding: chunked\r\n").into_bytes();
                        for part in b.chunks(1000) {
                            out.extend_from_slice(format!("{:x}\r\n", part.len()).as_bytes());
                            out.extend_from_slice(part);
                            out.extend_from_slice(b"\r\n");
                        }
                        out.extend_from_slice(b"0\r\n\r\n");
                        conn.write_all(&out)
                    }
                    Some(Reply::Redirect(to)) => conn.write_all(
                        head(
                            "302 Found",
                            &format!("Location: {to}\r\nContent-Length: 0\r\n"),
                        )
                        .as_bytes(),
                    ),
                    None => {
                        conn.write_all(head("404 Not Found", "Content-Length: 0\r\n").as_bytes())
                    }
                };
            }
        });
        Releases { base, routes }
    }

    fn route(&self, path: &str, reply: Reply) {
        self.routes
            .lock()
            .unwrap()
            .insert(format!("/releases{path}"), reply);
    }

    fn publish(&self, version: &str, binary: &[u8], tamper: bool) -> String {
        let (platform, ext) = burrow::update::platform().expect("a release platform");
        let asset = format!("burrow-{version}-{platform}.{ext}");
        let archive = archive(&format!("burrow-{version}-{platform}"), binary, ext);
        let mut sum: String = Sha256::digest(&archive)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if tamper {
            sum.replace_range(..1, if sum.starts_with('0') { "1" } else { "0" });
        }
        let sums = format!(
            "{}  burrow-{version}-other-arch.tar.gz\n{sum}  {asset}\n",
            "f".repeat(64)
        );
        self.route(
            &format!("/download/v{version}/SHA256SUMS"),
            Reply::Chunked(sums.into_bytes()),
        );
        self.route(
            &format!("/download/v{version}/{asset}"),
            Reply::Body(archive),
        );
        self.route(
            "/latest/download/SHA256SUMS",
            Reply::Redirect(format!("/releases/download/v{version}/SHA256SUMS")),
        );
        asset
    }
}

fn archive(folder: &str, binary: &[u8], ext: &str) -> Vec<u8> {
    let exe = if ext == "zip" { "burrow.exe" } else { "burrow" };
    let name = format!("{folder}/{exe}");
    if ext == "zip" {
        let mut out = Vec::new();
        out.extend_from_slice(&[
            0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        out.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        out.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(binary);
        let start = out.len() as u32;
        let mut central = vec![
            0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        central.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        central.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 16]);
        central.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&central);
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 1, 0, 1, 0]);
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        return out;
    }
    let mut tar = Vec::new();
    let mut entry = |path: &str, body: &[u8], kind: u8| {
        let mut h = [0u8; 512];
        h[..path.len()].copy_from_slice(path.as_bytes());
        h[100..107].copy_from_slice(b"0000755");
        h[124..135].copy_from_slice(format!("{:011o}", body.len()).as_bytes());
        h[156] = kind;
        h[257..262].copy_from_slice(b"ustar");
        tar.extend_from_slice(&h);
        tar.extend_from_slice(body);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    };
    entry(&format!("{folder}/"), b"", b'5');
    entry(&format!("{folder}/LICENSE"), b"MIT", b'0');
    entry(&name, binary, b'0');
    tar.resize(tar.len() + 1024, 0);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

struct Install {
    _dir: tempfile::TempDir,
    binary: PathBuf,
    original: Vec<u8>,
}

fn install() -> Install {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join(if cfg!(windows) {
        "burrow.exe"
    } else {
        "burrow"
    });
    std::fs::copy(env!("CARGO_BIN_EXE_burrow"), &binary).unwrap();
    let original = std::fs::read(&binary).unwrap();
    Install {
        _dir: dir,
        binary,
        original,
    }
}

fn update(binary: &Path, releases: &Releases, args: &[&str]) -> Output {
    update_with(binary, releases, args, &[])
}

fn update_with(binary: &Path, releases: &Releases, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(binary);
    cmd.arg("update")
        .args(args)
        .env("BURROW_UPDATE_URL", &releases.base);
    for var in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        cmd.env_remove(var);
    }
    cmd.envs(env.iter().copied());
    cmd.output().unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("burrow-update") || n.ends_with(".burrow-new"))
        .collect()
}

#[test]
fn check_reports_a_newer_release_without_downloading() {
    let releases = Releases::start();
    releases.publish("99.0.0", b"not downloaded", false);
    let i = install();
    let out = update(&i.binary, &releases, &["--check"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains(&format!("burrow 99.0.0 is available (this is {MINE})")),
        "{}",
        text(&out)
    );
    assert_eq!(std::fs::read(&i.binary).unwrap(), i.original);
}

#[test]
fn the_same_version_is_left_alone() {
    let releases = Releases::start();
    releases.publish(MINE, b"unused", false);
    let i = install();
    let out = update(&i.binary, &releases, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("is up to date, nothing to do"),
        "{}",
        text(&out)
    );
    assert_eq!(std::fs::read(&i.binary).unwrap(), i.original);
}

#[test]
fn replaces_this_binary_after_verifying_the_download() {
    let releases = Releases::start();
    let mut new = std::fs::read(env!("CARGO_BIN_EXE_burrow")).unwrap();
    new.extend_from_slice(b"a newer build");
    let asset = releases.publish(MINE, &new, false);
    let i = install();
    let out = update(&i.binary, &releases, &["--force"]);
    assert!(out.status.success(), "{}", text(&out));
    let said = text(&out);
    assert!(said.contains(&format!("downloading {asset}")), "{said}");
    assert!(said.contains(&format!("verified {asset}")), "{said}");
    assert!(said.contains(&format!("{MINE} -> {MINE}")), "{said}");
    assert_eq!(std::fs::read(&i.binary).unwrap(), new);
    let version = Command::new(&i.binary).arg("version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        format!("burrow {MINE}")
    );
    assert!(leftovers(i.binary.parent().unwrap()).is_empty());
}

#[test]
fn a_download_that_does_not_match_its_checksum_changes_nothing() {
    let releases = Releases::start();
    releases.publish(MINE, b"tampered", true);
    let i = install();
    let out = update(&i.binary, &releases, &["--force"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("does not match its SHA-256"),
        "{}",
        text(&out)
    );
    assert_eq!(std::fs::read(&i.binary).unwrap(), i.original);
    assert!(leftovers(i.binary.parent().unwrap()).is_empty());
}

#[test]
fn a_binary_that_reports_another_version_is_refused() {
    let releases = Releases::start();
    let real = std::fs::read(env!("CARGO_BIN_EXE_burrow")).unwrap();
    releases.publish("99.0.0", &real, false);
    let i = install();
    let out = update(&i.binary, &releases, &[]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains(&format!(
            "reports \"{MINE}\" instead of 99.0.0; nothing was changed"
        )),
        "{}",
        text(&out)
    );
    assert_eq!(std::fs::read(&i.binary).unwrap(), i.original);
    assert!(leftovers(i.binary.parent().unwrap()).is_empty());
}

#[test]
fn missing_releases_and_platforms_are_explained() {
    let releases = Releases::start();
    let i = install();
    let out = update(&i.binary, &releases, &[]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("no burrow release was found"),
        "{}",
        text(&out)
    );
    let out = update(&i.binary, &releases, &["--to", "v7.7.7"]);
    assert!(
        text(&out).contains("burrow 7.7.7 is not released"),
        "{}",
        text(&out)
    );
    releases.route(
        "/latest/download/SHA256SUMS",
        Reply::Body(format!("{}  burrow-99.0.0-plan9-mips.tar.gz\n", "a".repeat(64)).into_bytes()),
    );
    let out = update(&i.binary, &releases, &[]);
    assert!(text(&out).contains("the release has no"), "{}", text(&out));
    let out = update(&i.binary, &releases, &["--to", "soon"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert_eq!(std::fs::read(&i.binary).unwrap(), i.original);
}

fn connect_proxy() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let used = Arc::new(AtomicUsize::new(0));
    let count = used.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut down) = conn else { continue };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && down.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let line = String::from_utf8_lossy(&head)
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            let Some(target) = line
                .strip_prefix("CONNECT ")
                .and_then(|r| r.split(' ').next())
            else {
                continue;
            };
            let Ok(mut up) = std::net::TcpStream::connect(target) else {
                continue;
            };
            count.fetch_add(1, Ordering::SeqCst);
            down.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .unwrap();
            let (mut d2, mut u2) = (down.try_clone().unwrap(), up.try_clone().unwrap());
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut d2, &mut u2);
                let _ = u2.shutdown(std::net::Shutdown::Write);
            });
            let _ = std::io::copy(&mut up, &mut down);
            let _ = down.shutdown(std::net::Shutdown::Write);
        }
    });
    (url, used)
}

#[test]
fn downloads_go_through_the_proxy_from_the_environment() {
    let releases = Releases::start();
    let mut new = std::fs::read(env!("CARGO_BIN_EXE_burrow")).unwrap();
    new.extend_from_slice(b"through a proxy");
    releases.publish(MINE, &new, false);
    let (proxy, used) = connect_proxy();
    let i = install();
    let out = update_with(
        &i.binary,
        &releases,
        &["--force"],
        &[("HTTP_PROXY", &proxy)],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(std::fs::read(&i.binary).unwrap(), new);
    assert!(
        used.load(Ordering::SeqCst) >= 3,
        "the proxy was used {} times",
        used.load(Ordering::SeqCst)
    );
    let before = used.load(Ordering::SeqCst);
    let host = releases
        .base
        .split('/')
        .nth(2)
        .unwrap()
        .split(':')
        .next()
        .unwrap();
    let out = update_with(
        &i.binary,
        &releases,
        &["--check"],
        &[("HTTP_PROXY", &proxy), ("NO_PROXY", host)],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(used.load(Ordering::SeqCst), before, "NO_PROXY was ignored");
    let out = update_with(
        &i.binary,
        &releases,
        &["--check"],
        &[("HTTP_PROXY", "http://127.0.0.1:1")],
    );
    assert!(!out.status.success());
    assert!(
        text(&out).contains("through proxy http://127.0.0.1:1"),
        "{}",
        text(&out)
    );
}
