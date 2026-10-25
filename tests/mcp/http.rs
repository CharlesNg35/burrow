use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use super::common::admin::world;
use super::{Visitor, visitor};

pub struct Http {
    _child: tokio::process::Child,
    pub port: u16,
    pub token: String,
    pub stderr: String,
}

async fn start(v: &Visitor, args: &[&str]) -> Http {
    start_on(v, "0", args).await
}

pub async fn start_on(v: &Visitor, port: &str, args: &[&str]) -> Http {
    let mut all = vec!["--http", "--port", port];
    all.extend_from_slice(args);
    let quiet = args.contains(&"--no-print-token");
    let mut child = v
        .command(&all)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut seen = String::new();
    let last = if quiet { "  URL:" } else { "  Cursor:" };
    let started = tokio::time::timeout(Duration::from_secs(20), async {
        while let Ok(Some(line)) = lines.next_line().await {
            seen.push_str(&line);
            seen.push('\n');
            if line.starts_with(last) {
                return true;
            }
        }
        false
    })
    .await;
    assert!(matches!(started, Ok(true)), "no start block: {seen}");
    let url = seen
        .lines()
        .find_map(|l| l.strip_prefix("  URL:    "))
        .expect("the URL is printed");
    let port = url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|r| r.strip_suffix("/mcp"))
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("{url}"));
    let token = seen
        .lines()
        .find_map(|l| l.strip_prefix("  Header: Authorization: Bearer "))
        .unwrap_or_default()
        .to_string();
    Http {
        _child: child,
        port,
        token,
        stderr: seen,
    }
}

struct Answer {
    status: u16,
    head: String,
    body: String,
}

async fn raw(port: u16, method: &str, headers: &[(&str, String)], body: &str) -> Answer {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut req = format!(
        "{method} /mcp HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        req.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    s.write_all(body.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(20), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    Answer {
        status,
        head: head.to_ascii_lowercase(),
        body: body.to_string(),
    }
}

fn modern(method: &str, version: &str) -> String {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": method,
        "params": { "_meta": {
            "io.modelcontextprotocol/protocolVersion": version,
            "io.modelcontextprotocol/clientInfo": { "name": "tests", "version": "1" },
            "io.modelcontextprotocol/clientCapabilities": {}
        } }
    })
    .to_string()
}

fn mcp_headers(h: &Http, method: &str, version: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Authorization", format!("Bearer {}", h.token)),
        ("Content-Type", "application/json".into()),
        ("Accept", "application/json, text/event-stream".into()),
        ("MCP-Protocol-Version", version.into()),
        ("Mcp-Method", method.into()),
    ]
}

pub async fn list_with(port: u16, auth: &str) -> u16 {
    let headers = vec![
        ("Authorization", auth.to_string()),
        ("Content-Type", "application/json".into()),
        ("Accept", "application/json, text/event-stream".into()),
        ("MCP-Protocol-Version", "2026-07-28".into()),
        ("Mcp-Method", "tools/list".into()),
    ];
    raw(port, "POST", &headers, &modern("tools/list", "2026-07-28"))
        .await
        .status
}

fn tools_in(body: &str) -> Vec<String> {
    let json = body
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .unwrap_or(body);
    let v: Value = serde_json::from_str(json).unwrap_or_else(|_| panic!("{body}"));
    let mut names: Vec<String> = v["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("{body}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn http_needs_the_token_a_local_origin_and_a_post() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let h = start(
        &v,
        &["--token-file", v.dir.path().join("t").to_str().unwrap()],
    )
    .await;
    let good = mcp_headers(&h, "tools/list", "2026-07-28");
    let list = modern("tools/list", "2026-07-28");
    let ok = raw(h.port, "POST", &good, &list).await;
    assert_eq!(ok.status, 200, "{}", ok.body);
    assert_eq!(tools_in(&ok.body).len(), 4, "{}", ok.body);
    let mut none = good.clone();
    none.retain(|(k, _)| *k != "Authorization");
    assert_eq!(raw(h.port, "POST", &none, &list).await.status, 401);
    let mut wrong = none.clone();
    wrong.push(("Authorization", format!("Bearer {}0", h.token)));
    let refused = raw(h.port, "POST", &wrong, &list).await;
    assert_eq!(refused.status, 401);
    assert!(
        refused.head.contains("www-authenticate: bearer"),
        "{}",
        refused.head
    );
    for origin in ["http://evil.example", "http://127.0.0.1:1", "null"] {
        let mut o = good.clone();
        o.push(("Origin", origin.into()));
        assert_eq!(raw(h.port, "POST", &o, &list).await.status, 403, "{origin}");
    }
    let mut local = good.clone();
    local.push(("Origin", format!("http://localhost:{}", h.port)));
    assert_eq!(raw(h.port, "POST", &local, &list).await.status, 200);
    let mut host = good.clone();
    host.push(("Host", "evil.example".into()));
    assert_eq!(raw(h.port, "POST", &host, &list).await.status, 403);
    for method in ["GET", "DELETE"] {
        assert_eq!(raw(h.port, method, &good, "").await.status, 405, "{method}");
    }
    let big = "x".repeat(1024 * 1024 + 1);
    assert_eq!(raw(h.port, "POST", &good, &big).await.status, 413);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn the_protocol_version_header_must_match_and_be_known() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let h = start(&v, &[]).await;
    let list = modern("tools/list", "2026-07-28");
    let mismatch = raw(
        h.port,
        "POST",
        &mcp_headers(&h, "tools/list", "2025-11-25"),
        &list,
    )
    .await;
    assert_eq!(mismatch.status, 400, "{}", mismatch.body);
    let unknown = modern("tools/list", "1999-01-01");
    let unknown = raw(
        h.port,
        "POST",
        &mcp_headers(&h, "tools/list", "1999-01-01"),
        &unknown,
    )
    .await;
    assert_eq!(unknown.status, 400, "{}", unknown.body);
    let mut session = mcp_headers(&h, "tools/list", "2026-07-28");
    session.push(("Mcp-Session-Id", "made-up".into()));
    let ignored = raw(h.port, "POST", &session, &list).await;
    assert_eq!(ignored.status, 200, "{}", ignored.body);
    assert!(!ignored.head.contains("mcp-session-id"), "{}", ignored.head);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_2025_11_25_client_works_without_a_session() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let h = start(&v, &[]).await;
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-11-25", "capabilities": {},
                    "clientInfo": { "name": "old", "version": "1" } }
    })
    .to_string();
    let mut headers = mcp_headers(&h, "initialize", "2025-11-25");
    headers.retain(|(k, _)| *k != "MCP-Protocol-Version");
    let answer = raw(h.port, "POST", &headers, &init).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(answer.body.contains("2025-11-25"), "{}", answer.body);
    assert!(!answer.head.contains("mcp-session-id"), "{}", answer.head);
    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string();
    let listed = raw(
        h.port,
        "POST",
        &mcp_headers(&h, "tools/list", "2025-11-25"),
        &list,
    )
    .await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    assert_eq!(tools_in(&listed.body).len(), 4, "{}", listed.body);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn the_token_is_kept_per_config_privately_and_shown_unless_asked_not_to() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let first = start(&v, &[]).await;
    assert_eq!(first.token.len(), 64, "{}", first.stderr);
    assert!(
        first.stderr.contains("Claude Code:  burrow mcp -c "),
        "{}",
        first.stderr
    );
    assert!(
        !first.stderr.contains(".token"),
        "the file is not named: {}",
        first.stderr
    );
    drop(first);
    let quiet = start(&v, &["--no-print-token"]).await;
    assert!(
        quiet.stderr.ends_with(&format!(
            "burrow mcp: MCP over HTTP\n  URL:    http://127.0.0.1:{}/mcp\n",
            quiet.port
        )),
        "{}",
        quiet.stderr
    );
    assert!(
        quiet.token.is_empty() && !quiet.stderr.contains("Bearer"),
        "{}",
        quiet.stderr
    );
    let files: Vec<_> = std::fs::read_dir(v.sessions())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "token"))
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let kept = std::fs::read_to_string(&files[0])
        .unwrap()
        .trim()
        .to_string();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&files[0]).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let printed = v.command(&["--print-token"]).output().await.unwrap();
    assert_eq!(String::from_utf8_lossy(&printed.stdout).trim(), kept);
    let list = modern("tools/list", "2026-07-28");
    let mut headers = mcp_headers(&quiet, "tools/list", "2026-07-28");
    headers[0].1 = format!("Bearer {kept}");
    assert_eq!(raw(quiet.port, "POST", &headers, &list).await.status, 200);
    let renewed = v
        .command(&["--new-token", "--print-token"])
        .output()
        .await
        .unwrap();
    let fresh = String::from_utf8_lossy(&renewed.stdout).trim().to_string();
    assert_ne!(fresh, kept);
    assert_eq!(
        raw(quiet.port, "POST", &headers, &list).await.status,
        401,
        "the old token stops at once"
    );
    headers[0].1 = format!("Bearer {fresh}");
    assert_eq!(raw(quiet.port, "POST", &headers, &list).await.status, 200);
    burrow::base::fs::write_private(&files[0], &format!("{fresh}x\n")).unwrap();
    assert_eq!(
        raw(quiet.port, "POST", &headers, &list).await.status,
        401,
        "a malformed file lets nobody in"
    );
    std::fs::remove_file(&files[0]).unwrap();
    assert_eq!(
        raw(quiet.port, "POST", &headers, &list).await.status,
        401,
        "a missing file lets nobody in"
    );
    headers[0].1 = "Bearer ".into();
    assert_eq!(raw(quiet.port, "POST", &headers, &list).await.status, 401);
    let _ = w.stop.send(true);
}
