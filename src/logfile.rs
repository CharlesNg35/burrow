use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const KEEP: usize = 5;

struct State {
    file: File,
    size: u64,
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
        let size = file.metadata()?.len();
        Ok(RotatingFile {
            path: Arc::new(path.to_path_buf()),
            state: Arc::new(Mutex::new(State { file, size })),
            max,
        })
    }

    fn rotated(&self, n: usize) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    fn rotate(&self, state: &mut State) -> io::Result<()> {
        for n in (1..KEEP).rev() {
            let _ = std::fs::rename(self.rotated(n), self.rotated(n + 1));
        }
        state.file.flush()?;
        std::fs::rename(&*self.path, self.rotated(1))?;
        state.file = open_log(&self.path)?;
        state.size = 0;
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("log file lock poisoned"))?;
        if state.size + buf.len() as u64 > self.max && state.size > 0 {
            self.rotate(&mut state)?;
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
}
