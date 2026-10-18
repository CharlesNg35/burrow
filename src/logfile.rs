use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const KEEP: usize = 5;

struct State {
    file: File,
    size: u64,
    rotate_at: u64,
    id: Option<(u64, u64)>,
    checked: Instant,
}

const CHECK_EVERY: Duration = Duration::from_secs(1);

#[cfg(unix)]
fn id_of(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn id_of(_meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

fn open_log(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o640);
    }
    opts.open(path)
}

#[derive(Clone)]
pub struct RotatingFile {
    path: Arc<PathBuf>,
    state: Arc<Mutex<State>>,
    max: u64,
}

impl RotatingFile {
    pub fn open(path: &Path) -> io::Result<RotatingFile> {
        Self::with_limit(path, MAX_BYTES)
    }

    pub fn with_limit(path: &Path, max: u64) -> io::Result<RotatingFile> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let file = open_log(path)?;
        let meta = file.metadata()?;
        Ok(RotatingFile {
            path: Arc::new(path.to_path_buf()),
            state: Arc::new(Mutex::new(State {
                file,
                size: meta.len(),
                rotate_at: max,
                id: id_of(&meta),
                checked: Instant::now(),
            })),
            max,
        })
    }

    fn rotated(&self, n: usize) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    fn rotate(&self, state: &mut State) -> io::Result<()> {
        state.file.flush()?;
        if self.path.exists() {
            let aside = self.rotated(0);
            std::fs::rename(&*self.path, &aside)?;
            for n in (1..KEEP).rev() {
                let _ = std::fs::rename(self.rotated(n), self.rotated(n + 1));
            }
            let _ = std::fs::rename(&aside, self.rotated(1));
        }
        state.file = open_log(&self.path)?;
        state.size = 0;
        state.rotate_at = self.max;
        state.id = state.file.metadata().ok().as_ref().and_then(id_of);
        Ok(())
    }

    fn follow(&self, state: &mut State) {
        if state.checked.elapsed() < CHECK_EVERY {
            return;
        }
        state.checked = Instant::now();
        let moved = match std::fs::metadata(&*self.path) {
            Ok(meta) => state.id.is_some() && id_of(&meta) != state.id,
            Err(_) => true,
        };
        if !moved {
            return;
        }
        let _ = state.file.flush();
        if let Ok(file) = open_log(&self.path) {
            let meta = file.metadata().ok();
            state.size = meta.as_ref().map_or(0, std::fs::Metadata::len);
            state.id = meta.as_ref().and_then(id_of);
            state.rotate_at = self.max;
            state.file = file;
        }
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("log file lock poisoned"))?;
        self.follow(&mut state);
        if state.size + buf.len() as u64 > state.rotate_at
            && state.size > 0
            && self.rotate(&mut state).is_err()
        {
            state.rotate_at = state.size + (self.max / 10).max(1);
        }
        state.file.write_all(buf)?;
        state.size += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("log file lock poisoned"))?;
        state.file.flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RotatingFile {
    type Writer = RotatingFile;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_and_keeps_a_bounded_number_of_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("burrow.log");
        let mut log = RotatingFile::with_limit(&path, 100).unwrap();
        for i in 0..50 {
            writeln!(log, "line {i:03} padding padding").unwrap();
        }
        assert!(std::fs::metadata(&path).unwrap().len() <= 100);
        for n in 1..=KEEP {
            assert!(
                dir.path().join(format!("burrow.log.{n}")).exists(),
                "keeps .{n}"
            );
        }
        assert!(
            !dir.path().join(format!("burrow.log.{}", KEEP + 1)).exists(),
            "older files are dropped"
        );
        let newest = std::fs::read_to_string(&path).unwrap();
        assert!(newest.contains("line 049"));
    }

    #[cfg(unix)]
    #[test]
    fn a_log_deleted_or_moved_away_is_reopened_long_before_it_is_full() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("burrow.log");
        let mut log = RotatingFile::open(&path).unwrap();
        writeln!(log, "first").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::thread::sleep(CHECK_EVERY + Duration::from_millis(50));
        writeln!(log, "after delete").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after delete\n");
        let moved = dir.path().join("burrow.log.old");
        std::fs::rename(&path, &moved).unwrap();
        std::thread::sleep(CHECK_EVERY + Duration::from_millis(50));
        writeln!(log, "after move").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after move\n");
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "after delete\n");
        assert!(!dir.path().join("burrow.log.1").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_deleted_log_is_recreated_without_shifting_the_old_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("burrow.log");
        let mut log = RotatingFile::with_limit(&path, 100).unwrap();
        for i in 0..40 {
            writeln!(log, "line {i:03} padding padding").unwrap();
        }
        let old =
            |n: usize| std::fs::read_to_string(dir.path().join(format!("burrow.log.{n}"))).unwrap();
        let before: Vec<String> = (1..=KEEP).map(old).collect();
        std::fs::remove_file(&path).unwrap();
        writeln!(log, "after the delete").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "after the delete\n"
        );
        let after: Vec<String> = (1..=KEEP).map(old).collect();
        assert_eq!(before, after, "old logs are not shifted");
    }

    #[cfg(unix)]
    #[test]
    fn lines_are_kept_when_rotation_fails() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        std::fs::create_dir(&logs).unwrap();
        let path = logs.join("burrow.log");
        let mut log = RotatingFile::with_limit(&path, 100).unwrap();
        std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o500)).unwrap();
        let blocked = std::fs::rename(&path, logs.join("probe")).is_err();
        for i in 0..40 {
            writeln!(log, "line {i:03} padding padding").unwrap();
        }
        std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o700)).unwrap();
        if blocked {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                text.contains("line 000") && text.contains("line 039"),
                "{text}"
            );
        }
    }
}
