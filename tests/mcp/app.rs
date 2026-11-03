use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

pub const ANSWER: Duration = Duration::from_secs(20);
pub const STOP: Duration = Duration::from_secs(1);

pub struct App {
    pub child: Child,
    stdin: Option<ChildStdin>,
    out: Lines<BufReader<ChildStdout>>,
    err: Arc<Mutex<String>>,
    pub lines: Vec<String>,
    next: u64,
    _cwd: tempfile::TempDir,
}

fn minimal_env(home: &Path) -> Vec<(String, String)> {
    let mut env = vec![("HOME".to_string(), home.display().to_string())];
    if cfg!(windows) {
        for k in [
            "SystemRoot",
            "windir",
            "SYSTEMDRIVE",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
        ] {
            if let Ok(v) = std::env::var(k) {
                env.push((k.to_string(), v));
            }
        }
    } else {
        env.push(("PATH".into(), "/usr/bin:/bin".into()));
    }
    env
}

pub fn launch(args: &[&str], state: &Path) -> App {
    let cwd = tempfile::tempdir().unwrap();
    let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_burrow"));
    c.args(args)
        .current_dir(cwd.path())
        .env_clear()
        .envs(minimal_env(cwd.path()))
        .env("BURROW_ADMIN_SESSION_DIR", state)
        .env("BURROW_INSTANCE_DIR", state.join("instances"))
        .env("BURROW_SERVICE_DIR", state.join("services"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = c.spawn().unwrap();
    let err = Arc::new(Mutex::new(String::new()));
    let mut stderr = child.stderr.take().unwrap();
    let kept = err.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 {
                break;
            }
            kept.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    App {
        stdin: child.stdin.take(),
        out: BufReader::new(child.stdout.take().unwrap()).lines(),
        child,
        err,
        lines: Vec::new(),
        next: 1,
        _cwd: cwd,
    }
}

pub fn mcp(config: &Path, extra: &[&str], state: &Path) -> App {
    let config = config.display().to_string();
    let mut args = vec!["mcp", "-c", config.as_str()];
    args.extend_from_slice(extra);
    launch(&args, state)
}

impl App {
    pub fn stderr(&self) -> String {
        self.err.lock().unwrap().clone()
    }

    pub async fn send(&mut self, v: Value) {
        let mut line = serde_json::to_vec(&v).unwrap();
        line.push(b'\n');
        let stdin = self.stdin.as_mut().expect("stdin is open");
        stdin.write_all(&line).await.unwrap();
        stdin.flush().await.unwrap();
    }

    pub async fn recv(&mut self) -> Option<Value> {
        let line = tokio::time::timeout(ANSWER, self.out.next_line())
            .await
            .unwrap_or_else(|_| panic!("no answer in {ANSWER:?}; stderr: {}", self.stderr()))
            .unwrap()?;
        let v: Value = serde_json::from_str(&line)
            .unwrap_or_else(|_| panic!("stdout carried something else than JSON-RPC: {line:?}"));
        assert_eq!(v["jsonrpc"], "2.0", "{line}");
        self.lines.push(line);
        Some(v)
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        loop {
            let v = self
                .recv()
                .await
                .unwrap_or_else(|| panic!("stdout closed; stderr: {}", self.stderr()));
            if v["id"] == id {
                return v;
            }
        }
    }

    pub async fn initialize(&mut self) -> Value {
        let v = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "desktop-app", "version": "1.0" }
                }),
            )
            .await;
        if v.get("result").is_some() {
            self.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .await;
        }
        v
    }

    pub async fn ready(&mut self) -> Value {
        let v = self.initialize().await;
        assert!(v["result"].is_object(), "{v}; stderr: {}", self.stderr());
        v["result"].clone()
    }

    pub async fn tools(&mut self) -> Vec<String> {
        let v = self.request("tools/list", json!({})).await;
        let mut names: Vec<String> = v["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("{v}"))
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    }

    pub async fn call(&mut self, name: &str, args: Value) -> (bool, Value) {
        let v = self
            .request("tools/call", json!({ "name": name, "arguments": args }))
            .await;
        let r = &v["result"];
        (
            r["isError"].as_bool().unwrap_or(false),
            r["structuredContent"].clone(),
        )
    }

    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    #[cfg(unix)]
    pub fn signal(&self, sig: rustix::process::Signal) {
        let pid = rustix::process::Pid::from_raw(self.child.id().unwrap() as i32).unwrap();
        rustix::process::kill_process(pid, sig).unwrap();
    }

    pub async fn exits_within(&mut self, limit: Duration) -> ExitStatus {
        let started = Instant::now();
        let status = tokio::time::timeout(limit + Duration::from_secs(10), self.child.wait())
            .await
            .unwrap_or_else(|_| panic!("burrow mcp did not stop; stderr: {}", self.stderr()))
            .unwrap();
        let took = started.elapsed();
        assert!(
            took < limit,
            "burrow mcp took {took:?} to stop; stderr: {}",
            self.stderr()
        );
        status
    }

    pub async fn rest_of_stdout(&mut self) {
        while let Ok(Ok(Some(line))) =
            tokio::time::timeout(Duration::from_secs(5), self.out.next_line()).await
        {
            let v: Value = serde_json::from_str(&line)
                .unwrap_or_else(|_| panic!("stdout carried something else: {line:?}"));
            assert_eq!(v["jsonrpc"], "2.0", "{line}");
            self.lines.push(line);
        }
    }
}

pub struct Relay {
    pub addr: std::net::SocketAddr,
    open: Arc<std::sync::atomic::AtomicBool>,
    cut: tokio::sync::watch::Sender<u64>,
}

pub async fn relay(to: std::net::SocketAddr) -> Relay {
    use std::sync::atomic::Ordering;
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let open = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (cut, cuts) = tokio::sync::watch::channel(0u64);
    let gate = open.clone();
    tokio::spawn(async move {
        while let Ok((mut a, _)) = l.accept().await {
            if !gate.load(Ordering::SeqCst) {
                drop(a);
                continue;
            }
            let mut cuts = cuts.clone();
            cuts.mark_unchanged();
            tokio::spawn(async move {
                let Ok(mut b) = tokio::net::TcpStream::connect(to).await else {
                    return;
                };
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut a, &mut b) => {}
                    _ = cuts.changed() => {}
                }
            });
        }
    });
    Relay { addr, open, cut }
}

impl Relay {
    pub fn cut(&self) {
        self.open.store(false, std::sync::atomic::Ordering::SeqCst);
        self.cut.send_modify(|n| *n += 1);
    }

    pub fn restore(&self) {
        self.open.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
