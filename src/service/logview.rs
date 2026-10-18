use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::failure::Failure;

fn rotated(path: &Path, n: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

pub(crate) fn open(path: &Path) -> std::io::Result<File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = opts.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(file)
}

fn read_lines(path: &Path) -> Vec<String> {
    open(path)
        .map(|f| BufReader::new(f).lines().map_while(Result::ok).collect())
        .unwrap_or_default()
}

pub fn last_lines(path: &Path, n: usize) -> Vec<String> {
    let mut lines = read_lines(path);
    let mut k = 1;
    while lines.len() < n && k <= crate::logfile::KEEP {
        let mut older = read_lines(&rotated(path, k));
        if older.is_empty() {
            break;
        }
        older.append(&mut lines);
        lines = older;
        k += 1;
    }
    let skip = lines.len().saturating_sub(n);
    lines.split_off(skip)
}

pub fn show(
    path: &Path,
    n: usize,
    follow: bool,
    retry: &str,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    match open(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(
                Failure::new(format!("no log file yet at {}", path.display())).hint(format!(
                    "the service creates it when it starts; check it with: {}",
                    crate::failure::cmd("service list")
                )),
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(Failure::new(format!(
                "cannot read {}: only administrators can",
                path.display()
            ))
            .hint(if cfg!(windows) {
                "run it from a terminal opened with \"Run as administrator\"".to_string()
            } else {
                format!("run: {retry}")
            }));
        }
        _ => {}
    }
    for line in last_lines(path, n) {
        if writeln!(out, "{line}").is_err() {
            return Ok(());
        }
    }
    if !follow {
        return Ok(());
    }
    let mut file =
        open(path).map_err(|e| Failure::new(format!("cannot open {}: {e}", path.display())))?;
    let mut pos = file.seek(SeekFrom::End(0)).unwrap_or(0);
    let mut buf = Vec::new();
    loop {
        std::thread::sleep(Duration::from_millis(300));
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if len < pos {
            let Ok(next) = open(path) else {
                continue;
            };
            buf.clear();
            if file.seek(SeekFrom::Start(pos)).is_ok()
                && file.read_to_end(&mut buf).is_ok()
                && out.write_all(&buf).and_then(|_| out.flush()).is_err()
            {
                return Ok(());
            }
            file = next;
            pos = 0;
        }
        buf.clear();
        if file.seek(SeekFrom::Start(pos)).is_ok()
            && file.read_to_end(&mut buf).is_ok()
            && !buf.is_empty()
        {
            pos += buf.len() as u64;
            if out.write_all(&buf).and_then(|_| out.flush()).is_err() {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_planted_link_or_pipe_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        std::fs::write(&secret, "private\n").unwrap();
        let linked = dir.path().join("a.log");
        std::os::unix::fs::symlink(&secret, &linked).unwrap();
        assert!(last_lines(&linked, 10).is_empty());
        let fifo = dir.path().join("b.log");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(last_lines(&fifo, 10).is_empty());
    }

    #[test]
    fn last_lines_span_rotated_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.log");
        std::fs::write(rotated(&p, 1), "a\nb\nc\n").unwrap();
        std::fs::write(&p, "d\ne\n").unwrap();
        assert_eq!(last_lines(&p, 3), vec!["c", "d", "e"]);
        assert_eq!(last_lines(&p, 1), vec!["e"]);
        assert_eq!(last_lines(&p, 10), vec!["a", "b", "c", "d", "e"]);
    }
}
