use std::io::Write;
use std::path::{Path, PathBuf};

pub const FILE: &str = "burrow-mcp.log";
pub const MAX_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone)]
pub struct Journal {
    path: PathBuf,
}

fn stamp(unix: u64) -> String {
    let secs = unix % 86_400;
    format!(
        "{} {:02}:{:02}:{:02} UTC",
        burrow_admin_proto::date::date(unix),
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn old(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        std::fs::rename(path, old(path))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(line.as_bytes())
}

impl Journal {
    pub fn at(path: PathBuf) -> Journal {
        Journal { path }
    }

    pub fn open(config: Option<&Path>) -> Option<Journal> {
        let beside = config
            .map(super::setup::absolute)
            .and_then(|c| c.parent().map(|d| d.join(FILE)));
        let private = crate::admin_link::store::dir().map(|d| {
            let _ = crate::admin_link::store::make_private_dir(&d);
            d.join(FILE)
        });
        beside
            .into_iter()
            .chain(private)
            .find(|p| append(p, "").is_ok())
            .map(Journal::at)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, what: &str) {
        let line = format!(
            "{} burrow mcp {} (pid {}): {what}\n",
            stamp(burrow_syntax::duration::now_secs()),
            crate::base::build::version(),
            std::process::id()
        );
        let _ = append(&self.path, &line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_sits_beside_the_config_and_rolls_over_when_large() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("visitor.yaml");
        std::fs::write(&config, "x").unwrap();
        let j = Journal::open(Some(&config)).unwrap();
        assert_eq!(
            dunce::canonicalize(j.path().parent().unwrap()).unwrap(),
            dunce::canonicalize(dir.path()).unwrap()
        );
        j.write("could not start: x");
        let text = std::fs::read_to_string(j.path()).unwrap();
        assert!(text.ends_with("could not start: x\n"), "{text}");
        assert!(text.contains(" UTC burrow mcp "), "{text}");
        std::fs::write(j.path(), vec![b'a'; MAX_BYTES as usize + 1]).unwrap();
        j.write("after");
        assert_eq!(
            std::fs::read_to_string(j.path()).unwrap().lines().count(),
            1
        );
        assert!(old(j.path()).exists());
        assert_eq!(stamp(86_400 + 3661), "2 Jan 1970 01:01:01 UTC");
    }
}
