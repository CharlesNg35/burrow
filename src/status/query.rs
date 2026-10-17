use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::Entry;
use super::render::{Paint, Rates, rates, render};
use crate::failure::Failure;
use crate::metrics::Status;

const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY: u64 = 16 * 1024 * 1024;

pub enum Pick {
    All,
    Config(PathBuf),
    Service(String),
}

pub struct Found {
    pub entry: Entry,
    pub status: Status,
}

pub struct Scan {
    pub found: Vec<Found>,
    pub hidden: usize,
}

pub fn fetch(entry: &Entry) -> Result<Status, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], entry.port));
    let mut s = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(TIMEOUT))
        .map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(TIMEOUT))
        .map_err(|e| e.to_string())?;
    let request = format!(
        "GET /status HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        entry.token
    );
    s.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.take(MAX_BODY)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("the answer is not HTTP")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    if !head.starts_with("HTTP/1.1 200") {
        return Err(format!(
            "it answered {}",
            head.lines().next().unwrap_or("nothing")
        ));
    }
    serde_json::from_slice(&raw[split + 4..]).map_err(|e| format!("not a burrow status: {e}"))
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn wanted(pick: &Pick) -> Result<Option<PathBuf>, Failure> {
    Ok(match pick {
        Pick::All => None,
        Pick::Config(p) => Some(canonical(p)),
        Pick::Service(name) => Some(canonical(&crate::service::resolve(Some(name))?.config)),
    })
}

pub fn scan(dir: &Path, wanted: Option<&Path>) -> Scan {
    let mut found = Vec::new();
    let mut hidden = 0;
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Scan { found, hidden };
    };
    let mut paths: Vec<PathBuf> = listing
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "status"))
        .collect();
    paths.sort();
    for path in paths {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                hidden += 1;
                continue;
            }
            Err(_) => continue,
        };
        let Ok(entry) = serde_json::from_str::<Entry>(&text) else {
            continue;
        };
        if wanted.is_some_and(|w| w != entry.config) {
            continue;
        }
        match fetch(&entry) {
            Ok(status) => found.push(Found { entry, status }),
            Err(_) => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    Scan { found, hidden }
}

fn none_running(pick: &Pick, hidden: usize) -> Failure {
    let what = match pick {
        Pick::All => "no burrow server is running on this machine".to_string(),
        Pick::Config(p) => format!("no running burrow server uses {}", p.display()),
        Pick::Service(n) => format!("service {n} is not running"),
    };
    let hint = if hidden > 0 {
        format!(
            "{hidden} running server(s) belong to another user, such as a service; run: {}",
            crate::failure::admin("status")
        )
    } else {
        format!(
            "burrow status shows servers; on a client or visitor, see {} and its logs",
            crate::failure::cmd("service status")
        )
    };
    Failure::new(what).hint(hint)
}

fn frame(scan: &Scan, paint: Paint, rates: Option<&HashMap<u32, Rates>>, all: bool) -> String {
    let many = scan.found.len() > 1;
    let mut text = String::new();
    for (i, f) in scan.found.iter().enumerate() {
        if many {
            if i > 0 {
                text.push('\n');
            }
            let title = format!("== {} (pid {})", f.entry.config.display(), f.entry.pid);
            text.push_str(&if paint.color {
                format!("\x1b[1m{title}\x1b[0m\n")
            } else {
                format!("{title}\n")
            });
        }
        let r = rates.and_then(|r| r.get(&f.entry.pid));
        text.push_str(&render(&f.status, paint, r));
    }
    if scan.hidden > 0 && all {
        text.push_str(&format!(
            "\n{} more running server(s) belong to another user; see them with: {}\n",
            scan.hidden,
            crate::failure::admin("status")
        ));
    }
    text
}

fn json(scan: &Scan) -> String {
    let list: Vec<serde_json::Value> = scan
        .found
        .iter()
        .map(|f| {
            serde_json::json!({
                "config": f.entry.config,
                "pid": f.entry.pid,
                "status": f.status,
            })
        })
        .collect();
    format!(
        "{}\n",
        serde_json::to_string_pretty(&list).expect("status serializes")
    )
}

pub struct Options {
    pub pick: Pick,
    pub json: bool,
    pub follow: Option<Duration>,
}

pub fn run(opts: Options, out: &mut dyn Write) -> Result<u8, Failure> {
    run_in(&crate::instance::default_dir(), opts, out)
}

pub fn run_in(dir: &Path, opts: Options, out: &mut dyn Write) -> Result<u8, Failure> {
    let wanted = wanted(&opts.pick)?;
    let all = matches!(opts.pick, Pick::All);
    let first = scan(dir, wanted.as_deref());
    if first.found.is_empty() {
        return Err(none_running(&opts.pick, first.hidden));
    }
    let tty = std::io::stdout().is_terminal();
    let paint = Paint {
        color: tty && crate::diagnostics::Style::detect(true).color,
    };
    let write = |out: &mut dyn Write, text: &str| {
        out.write_all(text.as_bytes())
            .and_then(|_| out.flush())
            .map_err(|e| Failure::new(format!("cannot write output: {e}")))
    };
    let Some(every) = opts.follow else {
        let text = if opts.json {
            json(&first)
        } else {
            frame(&first, paint, None, all)
        };
        write(out, &text)?;
        return Ok(0);
    };
    let mut previous = first;
    let mut taken = Instant::now();
    let shown = if opts.json {
        json(&previous)
    } else {
        frame(&previous, paint, None, all)
    };
    write(out, &clear(tty, &shown, every))?;
    loop {
        std::thread::sleep(every);
        let next = scan(dir, wanted.as_deref());
        let now = Instant::now();
        if next.found.is_empty() {
            write(out, "\nthe server stopped\n")?;
            return Ok(1);
        }
        let secs = now.duration_since(taken).as_secs_f64();
        let per_pid: HashMap<u32, Rates> = next
            .found
            .iter()
            .filter_map(|f| {
                let before = previous.found.iter().find(|p| p.entry.pid == f.entry.pid)?;
                Some((f.entry.pid, rates(&before.status, &f.status, secs)))
            })
            .collect();
        let text = if opts.json {
            json(&next)
        } else {
            frame(&next, paint, Some(&per_pid), all)
        };
        write(out, &clear(tty, &text, every))?;
        previous = next;
        taken = now;
    }
}

fn clear(tty: bool, text: &str, every: Duration) -> String {
    let footer = format!(
        "\nrefreshing every {}s, Ctrl-C to stop\n",
        every.as_secs_f64()
    );
    if tty {
        format!("\x1b[H\x1b[2J{text}{footer}")
    } else {
        format!("{text}\n")
    }
}
