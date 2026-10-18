use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::Entry;
use super::render::{Paint, Rates, rates, render};
use crate::failure::Failure;
use crate::metrics::Status;

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

const DEADLINE: Duration = Duration::from_secs(2);
const MAX_FILE: u64 = 4096;

enum Fetch {
    Gone,
    Failed,
}

pub fn fetch(entry: &Entry) -> Result<Status, String> {
    fetch_by(entry, Instant::now() + DEADLINE).map_err(|(_, why)| why)
}

fn fetch_by(entry: &Entry, deadline: Instant) -> Result<Status, (Fetch, String)> {
    let failed = |e: String| (Fetch::Failed, e);
    let left = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| failed("it did not answer in time".into()))
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], entry.port));
    let mut s = TcpStream::connect_timeout(&addr, left()?).map_err(|e| {
        let kind = if e.kind() == std::io::ErrorKind::ConnectionRefused {
            Fetch::Gone
        } else {
            Fetch::Failed
        };
        (kind, e.to_string())
    })?;
    s.set_write_timeout(Some(left()?))
        .map_err(|e| failed(e.to_string()))?;
    let request = format!(
        "GET /status HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        entry.token
    );
    s.write_all(request.as_bytes())
        .map_err(|e| failed(e.to_string()))?;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        s.set_read_timeout(Some(left()?))
            .map_err(|e| failed(e.to_string()))?;
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) if (raw.len() + n) as u64 <= MAX_BODY => raw.extend_from_slice(&chunk[..n]),
            Ok(_) => return Err(failed("the answer is too large".into())),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(failed(e.to_string())),
        }
    }
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| failed("the answer is not HTTP".into()))?;
    let head = String::from_utf8_lossy(&raw[..split]);
    if !head.starts_with("HTTP/1.1 200") {
        return Err(failed(format!(
            "it answered {}",
            head.lines().next().unwrap_or("nothing")
        )));
    }
    let mut status: Status = serde_json::from_slice(&raw[split + 4..])
        .map_err(|e| failed(format!("not a burrow status: {e}")))?;
    clean_status(&mut status);
    Ok(status)
}

pub fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

fn clean_status(s: &mut Status) {
    let all = |v: &mut Vec<String>| v.iter_mut().for_each(|x| *x = clean(x));
    let opt = |v: &mut Option<String>| {
        if let Some(x) = v {
            *x = clean(x);
        }
    };
    s.version = clean(&s.version);
    all(&mut s.listening);
    for c in &mut s.clients {
        c.name = clean(&c.name);
        opt(&mut c.from);
    }
    for v in &mut s.services {
        v.name = clean(&v.name);
        v.protocol = clean(&v.protocol);
        v.target = clean(&v.target);
        opt(&mut v.bind);
        opt(&mut v.mode);
        opt(&mut v.proxy_protocol);
        all(&mut v.clients);
        all(&mut v.visitors);
        for h in &mut v.health {
            h.client = clean(&h.client);
        }
        for a in &mut v.set_aside {
            a.client = clean(&a.client);
        }
    }
    for v in &mut s.visitors {
        v.name = clean(&v.name);
    }
}

enum Seen {
    Entry(Entry),
    Hidden,
    Stale,
    Skip,
}

fn alive(pid: u32) -> Option<bool> {
    if cfg!(windows) {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        return Some(String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\"")));
    }
    cfg!(target_os = "linux").then(|| Path::new(&format!("/proc/{pid}")).exists())
}

fn read_entry(path: &Path) -> Seen {
    let Some(pid) = path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<u32>().ok())
    else {
        return Seen::Skip;
    };
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return match alive(pid) {
                Some(false) => Seen::Skip,
                _ => Seen::Hidden,
            };
        }
        Err(_) => return Seen::Skip,
    };
    let Ok(meta) = file.metadata() else {
        return Seen::Skip;
    };
    if !meta.is_file() || meta.len() > MAX_FILE {
        return Seen::Skip;
    }
    if alive(pid) == Some(false) {
        return Seen::Stale;
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = std::fs::metadata(format!("/proc/{pid}")).map(|m| m.uid());
        if owner.ok() != Some(meta.uid()) {
            return Seen::Skip;
        }
    }
    let mut text = String::new();
    if file.take(MAX_FILE).read_to_string(&mut text).is_err() {
        return Seen::Skip;
    }
    match serde_json::from_str::<Entry>(&text) {
        Ok(e) if e.pid == pid && e.port != 0 && valid_token(&e.token) => Seen::Entry(e),
        _ => Seen::Skip,
    }
}

fn valid_token(t: &str) -> bool {
    t.len() == 48 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn wanted(pick: &Pick) -> Result<Option<PathBuf>, Failure> {
    Ok(match pick {
        Pick::All => None,
        Pick::Config(p) => Some(canonical(p)),
        Pick::Service(name) => {
            let m = crate::service::resolve(Some(name))?;
            if m.role != "server" {
                return Err(Failure::usage(format!(
                    "{} is a {} service, and burrow status shows servers only",
                    m.name, m.role
                ))
                .hint(format!(
                    "see it with: {}",
                    crate::failure::admin(&format!("service status {}", m.name))
                )));
            }
            Some(canonical(&m.config))
        }
    })
}

pub fn scan(dir: &Path, wanted: Option<&Path>) -> Scan {
    let mut hidden = 0;
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Scan {
            found: Vec::new(),
            hidden,
        };
    };
    let mut paths: Vec<PathBuf> = listing
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "status"))
        .collect();
    paths.sort();
    let mut entries = Vec::new();
    for path in paths {
        match read_entry(&path) {
            Seen::Entry(entry) if wanted.is_none_or(|w| w == entry.config) => {
                entries.push((path, entry));
            }
            Seen::Hidden => hidden += 1,
            Seen::Stale => {
                let _ = std::fs::remove_file(&path);
            }
            Seen::Entry(_) | Seen::Skip => {}
        }
    }
    let deadline = Instant::now() + DEADLINE;
    let fetched: Vec<_> = std::thread::scope(|scope| {
        let jobs: Vec<_> = entries
            .iter()
            .map(|(_, entry)| scope.spawn(move || fetch_by(entry, deadline)))
            .collect();
        jobs.into_iter()
            .map(|j| j.join().unwrap_or(Err((Fetch::Failed, "panicked".into()))))
            .collect()
    });
    let mut found = Vec::new();
    for ((path, entry), result) in entries.into_iter().zip(fetched) {
        match result {
            Ok(status) => found.push(Found { entry, status }),
            Err((Fetch::Gone, _)) => {
                let _ = std::fs::remove_file(&path);
            }
            Err((Fetch::Failed, _)) => {}
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
            let title = paint.clip(&format!(
                "== {} (pid {})",
                clean(&f.entry.config.display().to_string()),
                f.entry.pid
            ));
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

struct Follow<'a> {
    dir: &'a Path,
    wanted: Option<PathBuf>,
    last: Scan,
    rates: Option<HashMap<u32, Rates>>,
    taken: Instant,
}

impl Follow<'_> {
    fn refresh(&mut self) -> bool {
        let next = scan(self.dir, self.wanted.as_deref());
        if next.found.is_empty() {
            return false;
        }
        let now = Instant::now();
        let secs = now.duration_since(self.taken).as_secs_f64();
        let rates = next
            .found
            .iter()
            .filter_map(|f| {
                let before = self
                    .last
                    .found
                    .iter()
                    .find(|p| p.entry.pid == f.entry.pid)?;
                Some((f.entry.pid, rates(&before.status, &f.status, secs)))
            })
            .collect();
        self.rates = Some(rates);
        self.last = next;
        self.taken = now;
        true
    }
}

fn write(out: &mut dyn Write, text: &str) -> Result<(), Failure> {
    out.write_all(text.as_bytes())
        .and_then(|_| out.flush())
        .map_err(|e| Failure::new(format!("cannot write output: {e}")))
}

pub fn run_in(dir: &Path, opts: Options, out: &mut dyn Write) -> Result<u8, Failure> {
    let wanted = wanted(&opts.pick)?;
    let all = matches!(opts.pick, Pick::All);
    let first = scan(dir, wanted.as_deref());
    if first.found.is_empty() {
        return Err(none_running(&opts.pick, first.hidden));
    }
    let tty = std::io::stdout().is_terminal();
    let color = tty && crate::diagnostics::Style::detect(true).color;
    let paint = || Paint {
        color,
        boxed: tty && !opts.json,
        width: if tty { super::live::width() } else { None },
    };
    let show = |scan: &Scan, rates: Option<&HashMap<u32, Rates>>| {
        if opts.json {
            json(scan)
        } else {
            frame(scan, paint(), rates, all)
        }
    };
    let Some(every) = opts.follow else {
        write(out, &show(&first, None))?;
        return Ok(0);
    };
    let mut follow = Follow {
        dir,
        wanted,
        last: first,
        rates: None,
        taken: Instant::now(),
    };
    if tty && !opts.json {
        let footer = paint().dim(&format!(
            "refreshing every {}s · q to quit",
            every.as_secs_f64()
        ));
        let outcome = super::live::run(every, &footer, |fresh| {
            if fresh && !follow.refresh() {
                return None;
            }
            Some(show(&follow.last, follow.rates.as_ref()))
        })
        .map_err(|e| Failure::new(format!("cannot draw the live status: {e}")))?;
        return match outcome {
            super::live::Outcome::Quit => Ok(0),
            super::live::Outcome::Stopped(last) => {
                write(out, &format!("{last}\nthe server stopped\n"))?;
                Ok(1)
            }
        };
    }
    write(out, &format!("{}\n", show(&follow.last, None)))?;
    loop {
        std::thread::sleep(every);
        if !follow.refresh() {
            write(out, "the server stopped\n")?;
            return Ok(1);
        }
        write(
            out,
            &format!("{}\n", show(&follow.last, follow.rates.as_ref())),
        )?;
    }
}
