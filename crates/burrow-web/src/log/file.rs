use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use tracing::warn;

pub const QUEUE: usize = 4096;
const CHECK_EVERY: Duration = Duration::from_secs(1);

pub struct LogFile {
    path: PathBuf,
    tx: SyncSender<String>,
    dropped: AtomicU64,
}

impl std::fmt::Debug for LogFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogFile").field("path", &self.path).finish()
    }
}

fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
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

struct Writer {
    path: PathBuf,
    out: BufWriter<File>,
    checked: Instant,
}

impl Writer {
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
            Err(e) => warn!(
                "web: cannot open the access log {}: {e}",
                self.path.display()
            ),
        }
    }

    fn put(&mut self, line: &str) {
        self.reopen_if_moved();
        let wrote = self
            .out
            .write_all(line.as_bytes())
            .and_then(|()| self.out.write_all(b"\n"));
        if let Err(e) = wrote {
            warn!(
                "web: writing the access log {} failed: {e}",
                self.path.display()
            );
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
                }
                Err(RecvTimeoutError::Timeout) => self.reopen_if_moved(),
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
        let writer = Writer {
            path: path.to_path_buf(),
            out: BufWriter::new(file),
            checked: Instant::now(),
        };
        std::thread::Builder::new()
            .name("burrow-access-log".into())
            .spawn(move || writer.run(&rx))?;
        Ok(LogFile {
            path: path.to_path_buf(),
            tx,
            dropped: AtomicU64::new(0),
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

    #[test]
    fn a_log_that_cannot_be_opened_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let e = LogFile::open(&dir.path().join("missing/access.log")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
    }
}
