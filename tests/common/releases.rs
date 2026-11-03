#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

#[derive(Clone)]
pub enum Reply {
    Body(Vec<u8>),
    Chunked(Vec<u8>),
    Redirect(String),
}

pub struct Releases {
    pub base: String,
    routes: Arc<Mutex<HashMap<String, Reply>>>,
    pub seen: Arc<Mutex<Vec<String>>>,
}

pub fn resolve_dots(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            ".." => {
                out.pop();
            }
            "." => {}
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    if joined.starts_with('/') {
        joined
    } else {
        format!("/{joined}")
    }
}

impl Releases {
    pub fn start() -> Releases {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/releases", listener.local_addr().unwrap());
        let routes: Arc<Mutex<HashMap<String, Reply>>> = Arc::default();
        let shared = routes.clone();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen_all = seen.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let shared = shared.clone();
                let shared_seen = seen_all.clone();
                std::thread::spawn(move || {
                    let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut req = Vec::new();
                    let mut byte = [0u8; 1];
                    while !req.ends_with(b"\r\n\r\n") && conn.read(&mut byte).unwrap_or(0) == 1 {
                        req.push(byte[0]);
                    }
                    let text = String::from_utf8_lossy(&req);
                    let path = resolve_dots(text.split_whitespace().nth(1).unwrap_or(""));
                    shared_seen.lock().unwrap().push(path.clone());
                    let reply = shared.lock().unwrap().get(&path).cloned();
                    let head = |status: &str, extra: &str| {
                        format!("HTTP/1.1 {status}\r\n{extra}Connection: close\r\n\r\n")
                    };
                    let _ = match reply {
                        Some(Reply::Body(b)) => conn
                            .write_all(
                                head("200 OK", &format!("Content-Length: {}\r\n", b.len()))
                                    .as_bytes(),
                            )
                            .and_then(|_| conn.write_all(&b)),
                        Some(Reply::Chunked(b)) => {
                            let mut out =
                                head("200 OK", "Transfer-Encoding: chunked\r\n").into_bytes();
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
                        None => conn
                            .write_all(head("404 Not Found", "Content-Length: 0\r\n").as_bytes()),
                    };
                });
            }
        });
        Releases { base, routes, seen }
    }

    pub fn set_latest(&self, version: &str) {
        self.route(
            "/latest/download/SHA256SUMS",
            Reply::Redirect(format!("/releases/download/v{version}/SHA256SUMS")),
        );
    }

    pub fn route_at(&self, path: &str, reply: Reply) {
        self.routes.lock().unwrap().insert(path.to_string(), reply);
    }

    pub fn route(&self, path: &str, reply: Reply) {
        self.routes
            .lock()
            .unwrap()
            .insert(format!("/releases{path}"), reply);
    }

    pub fn publish(&self, version: &str, binary: &[u8], tamper: bool) -> String {
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
        self.set_latest(version);
        asset
    }
}

pub fn archive(folder: &str, binary: &[u8], ext: &str) -> Vec<u8> {
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
