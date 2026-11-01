use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behave {
    Answer,
    CloseOnReuse,
    CloseAlways,
    CloseAfterAnswer,
    CloseSometimes,
    Ntlm,
    HalfHeadOnReuse,
    OneAnswerPerConnection,
}

#[derive(Debug, Clone)]
pub struct Asked {
    pub conn: usize,
    pub proxy_source: Option<IpAddr>,
    pub real_ip: Option<IpAddr>,
    pub id: Option<String>,
}

#[derive(Default)]
pub struct Log {
    pub conns: AtomicUsize,
    pub asked: Mutex<Vec<Asked>>,
}

impl Log {
    pub fn conns(&self) -> usize {
        self.conns.load(Ordering::SeqCst)
    }

    pub fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }
}

pub struct Head {
    pub headers: Vec<(String, String)>,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }
}

async fn line<S: AsyncRead + Unpin>(s: &mut S) -> Option<String> {
    let mut out = Vec::new();
    let mut one = [0u8; 1];
    while !out.ends_with(b"\r\n") {
        match s.read(&mut one).await {
            Ok(1) => out.push(one[0]),
            _ => return None,
        }
    }
    out.truncate(out.len() - 2);
    Some(String::from_utf8_lossy(&out).into_owned())
}

pub async fn read_head<S: AsyncRead + Unpin>(s: &mut S) -> Option<Head> {
    let first = line(s).await?;
    first.split(' ').nth(1)?;
    let mut headers = Vec::new();
    loop {
        let l = line(s).await?;
        if l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':')?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let head = Head { headers };
    let length: usize = head
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    s.read_exact(&mut body).await.ok()?;
    Some(head)
}

async fn proxy_source(s: &mut TcpStream) -> Option<IpAddr> {
    let mut peek = [0u8; 6];
    let n = s.peek(&mut peek).await.ok()?;
    if &peek[..n] != b"PROXY " {
        return None;
    }
    let l = line(s).await?;
    l.split(' ').nth(2)?.parse().ok()
}

fn answer(text: &str, extra: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{extra}\r\n{text}",
        text.len()
    )
}

async fn serve(mut s: TcpStream, conn: usize, behave: Behave, log: Arc<Log>) {
    let source = proxy_source(&mut s).await;
    let mut served = 0usize;
    while let Some(head) = read_head(&mut s).await {
        served += 1;
        let asked = Asked {
            conn,
            proxy_source: source,
            real_ip: head.header("x-real-ip").and_then(|v| v.parse().ok()),
            id: head.header("x-id"),
        };
        log.asked.lock().unwrap().push(asked);
        let close = match behave {
            Behave::CloseOnReuse => served > 1,
            Behave::CloseAlways => true,
            Behave::CloseSometimes => (conn + served).is_multiple_of(3),
            Behave::Answer
            | Behave::CloseAfterAnswer
            | Behave::Ntlm
            | Behave::OneAnswerPerConnection => false,
            Behave::HalfHeadOnReuse => {
                if served > 1 {
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\n").await;
                    return;
                }
                false
            }
        };
        if close {
            return;
        }
        let reply = match behave {
            Behave::Ntlm if head.header("authorization").is_none() => {
                "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: NTLM\r\nContent-Length: 0\r\n\r\n"
                    .to_string()
            }
            _ => answer(&format!("conn {conn}"), ""),
        };
        if s.write_all(reply.as_bytes()).await.is_err() {
            return;
        }
        if behave == Behave::OneAnswerPerConnection {
            tokio::time::sleep(Duration::from_millis(20)).await;
            return;
        }
        if behave == Behave::CloseAfterAnswer {
            tokio::time::sleep(Duration::from_millis(50)).await;
            return;
        }
    }
}

pub async fn recording_app(behave: Behave) -> (SocketAddr, Arc<Log>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let log = Arc::new(Log::default());
    let kept = log.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            let conn = kept.conns.fetch_add(1, Ordering::SeqCst) + 1;
            tokio::spawn(serve(s, conn, behave, kept.clone()));
        }
    });
    (addr, log)
}

pub struct Keep {
    s: TcpStream,
}

impl Keep {
    pub async fn open(web: SocketAddr) -> Keep {
        Keep {
            s: TcpStream::connect(web).await.unwrap(),
        }
    }

    pub async fn behind_proxy(web: SocketAddr, visitor: SocketAddr) -> Keep {
        let mut s = TcpStream::connect(web).await.unwrap();
        let header = format!(
            "PROXY TCP4 {} 127.0.0.1 {} 80\r\n",
            visitor.ip(),
            visitor.port()
        );
        s.write_all(header.as_bytes()).await.unwrap();
        Keep { s }
    }

    pub async fn send(&mut self, method: &str, path: &str, extra: &str) -> Option<String> {
        let body = if method == "POST" { "data" } else { "" };
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: app.example.com\r\nContent-Length: {}\r\n{extra}\r\n{body}",
            body.len()
        );
        self.s.write_all(req.as_bytes()).await.ok()?;
        let first = tokio::time::timeout(Duration::from_secs(10), line(&mut self.s))
            .await
            .ok()??;
        let mut length = 0usize;
        loop {
            let l = line(&mut self.s).await?;
            if l.is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':')
                && k.eq_ignore_ascii_case("content-length")
            {
                length = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; length];
        self.s.read_exact(&mut body).await.ok()?;
        Some(first)
    }

    pub async fn get(&mut self, path: &str) -> String {
        self.send("GET", path, "").await.unwrap_or_default()
    }
}

pub fn status(line: &str) -> u16 {
    line.split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0)
}
