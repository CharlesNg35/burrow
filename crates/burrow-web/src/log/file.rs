use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use tracing::warn;

pub const QUEUE: usize = 4096;
const CHECK_EVERY: Duration = Duration::from_secs(1);

pub struct LogFile {
    path: PathBuf,
    tx: SyncSender<String>,
    dropped: Arc<AtomicU64>,
}

impl std::fmt::Debug for LogFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogFile").field("path", &self.path).finish()
    }
}

fn open(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o640);
    }
    opts.open(path)
}

#[cfg(unix)]
fn same_file(path: &Path, file: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(path), file.metadata()) {
        (Ok(a), Ok(b)) => a.ino() == b.ino() && a.dev() == b.dev(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(path: &Path, _file: &File) -> bool {
    path.exists()
}

const WARN_EVERY: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Gate {
    last: Option<Instant>,
    held: u64,
}

impl Gate {
    fn pass(&mut self, now: Instant) -> Option<u64> {
        match self.last {
            Some(t) if now.duration_since(t) < WARN_EVERY => {
                self.held += 1;
                None
            }
            _ => {
                self.last = Some(now);
                Some(std::mem::take(&mut self.held))
            }
        }
    }
}

fn more(held: u64) -> String {
    if held == 0 {
        String::new()
    } else {
        format!(" ({held} more since the last warning)")
    }
}

fn write_line(out: &mut impl Write, line: &str) -> io::Result<()> {
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")
}

fn put_line(
    out: &mut impl Write,
    line: &str,
    gate: &mut Gate,
    path: &Path,
    now: Instant,
) -> Option<String> {
    let e = write_line(out, line).err()?;
    let held = gate.pass(now)?;
    Some(format!(
        "web: writing the access log {} failed: {e}{}",
        path.display(),
        more(held)
    ))
}

struct Writer {
    path: PathBuf,
    out: BufWriter<File>,
    checked: Instant,
    reopen_gate: Gate,
    write_gate: Gate,
    drop_gate: Gate,
    dropped: Arc<AtomicU64>,
    dropped_told: u64,
}

impl Writer {
    fn new(path: &Path, file: File, dropped: Arc<AtomicU64>) -> Writer {
        Writer {
            path: path.to_path_buf(),
            out: BufWriter::new(file),
            checked: Instant::now(),
            reopen_gate: Gate::default(),
            write_gate: Gate::default(),
            drop_gate: Gate::default(),
            dropped,
            dropped_told: 0,
        }
    }

    fn reopen_if_moved(&mut self) {
        if self.checked.elapsed() < CHECK_EVERY {
            return;
        }
        self.checked = Instant::now();
        if same_file(&self.path, self.out.get_ref()) {
            return;
        }
        let _ = self.out.flush();
        match open(&self.path) {
            Ok(f) => self.out = BufWriter::new(f),
            Err(e) => {
                if let Some(held) = self.reopen_gate.pass(Instant::now()) {
                    warn!(
                        "web: cannot open the access log {}: {e}{}",
                        self.path.display(),
                        more(held)
                    );
                }
            }
        }
    }

    fn dropped_note(&mut self, now: Instant) -> Option<String> {
        let total = self.dropped.load(Ordering::Relaxed);
        let new = total.checked_sub(self.dropped_told).filter(|n| *n > 0)?;
        self.drop_gate.pass(now)?;
        self.dropped_told = total;
        Some(format!(
            "web: {new} access log lines were dropped because writing {} fell behind",
            self.path.display()
        ))
    }

    fn put(&mut self, line: &str) {
        self.reopen_if_moved();
        let now = Instant::now();
        if let Some(w) = put_line(&mut self.out, line, &mut self.write_gate, &self.path, now) {
            warn!("{w}");
        }
    }

    fn tell_dropped(&mut self) {
        if let Some(w) = self.dropped_note(Instant::now()) {
            warn!("{w}");
        }
    }

    fn run(mut self, rx: &Receiver<String>) {
        loop {
            match rx.recv_timeout(CHECK_EVERY) {
                Ok(line) => {
                    self.put(&line);
                    while let Ok(more) = rx.try_recv() {
                        self.put(&more);
                    }
                    let _ = self.out.flush();
                    self.tell_dropped();
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.reopen_if_moved();
                    self.tell_dropped();
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = self.out.flush();
                    return;
                }
            }
        }
    }
}

impl LogFile {
    pub fn open(path: &Path) -> io::Result<LogFile> {
        let file = open(path)?;
        let (tx, rx) = sync_channel(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer = Writer::new(path, file, dropped.clone());
        std::thread::Builder::new()
            .name("burrow-access-log".into())
            .spawn(move || writer.run(&rx))?;
        Ok(LogFile {
            path: path.to_path_buf(),
            tx,
            dropped,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, line: String) {
        match self.tx.try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(path: &Path, lines: usize) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            if text.lines().count() >= lines || Instant::now() > deadline {
                return text;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn lines_are_appended_and_a_moved_file_is_opened_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        std::fs::write(&path, "old\n").unwrap();
        let log = LogFile::open(&path).unwrap();
        log.write("one".into());
        log.write("two".into());
        assert_eq!(wait_for(&path, 3), "old\none\ntwo\n");
        let moved = dir.path().join("access.log.1");
        std::fs::rename(&path, &moved).unwrap();
        std::thread::sleep(CHECK_EVERY + Duration::from_millis(200));
        log.write("three".into());
        assert_eq!(wait_for(&path, 1), "three\n");
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "old\none\ntwo\n");
        assert_eq!(log.dropped(), 0);
    }

    struct Broken;

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("No space left on device"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failing_disk_warns_once_a_minute_with_the_count_it_held_back() {
        let start = Instant::now();
        let mut gate = Gate::default();
        let path = Path::new("access.log");
        let told: Vec<(u64, String)> = (0..150)
            .filter_map(|s| {
                let now = start + Duration::from_secs(s);
                put_line(&mut Broken, "line", &mut gate, path, now).map(|w| (s, w))
            })
            .collect();
        let at: Vec<u64> = told.iter().map(|(s, _)| *s).collect();
        assert_eq!(at, [0, 60, 120]);
        assert_eq!(
            told[0].1,
            "web: writing the access log access.log failed: No space left on device"
        );
        assert!(
            told[1].1.ends_with(" (59 more since the last warning)"),
            "{}",
            told[1].1
        );
        let mut fine = Vec::new();
        assert_eq!(put_line(&mut fine, "ok", &mut gate, path, start), None);
        assert_eq!(fine, b"ok\n");
    }

    #[test]
    fn dropped_lines_are_reported_at_most_once_a_minute() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let dropped = Arc::new(AtomicU64::new(0));
        let mut w = Writer::new(&path, open(&path).unwrap(), dropped.clone());
        let start = Instant::now();
        assert_eq!(w.dropped_note(start), None);
        dropped.store(7, Ordering::Relaxed);
        let first = w.dropped_note(start).unwrap();
        assert!(
            first.starts_with("web: 7 access log lines were dropped"),
            "{first}"
        );
        dropped.store(9, Ordering::Relaxed);
        assert_eq!(w.dropped_note(start + Duration::from_secs(30)), None);
        let later = w.dropped_note(start + Duration::from_secs(61)).unwrap();
        assert!(later.starts_with("web: 2 access log lines"), "{later}");
        assert_eq!(w.dropped_note(start + Duration::from_secs(200)), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_access_log_is_readable_by_its_owner_and_group_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let _log = LogFile::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
    }

    #[test]
    fn a_log_that_cannot_be_opened_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let e = LogFile::open(&dir.path().join("missing/access.log")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
    }
}
