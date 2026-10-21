use std::path::{Path, PathBuf};

use crate::failure::Failure;

pub struct LaunchFile(PathBuf);

impl LaunchFile {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for LaunchFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn page(url: &str) -> String {
    let u = escape(url);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta http-equiv=\"refresh\" content=\"0;url={u}\">\n<title>burrow admin</title>\n</head>\n<body>\n<p><a href=\"{u}\">Open burrow admin</a></p>\n</body>\n</html>\n"
    )
}

pub fn write(dir: &Path, url: &str) -> Result<LaunchFile, Failure> {
    crate::instance::prepare(dir).map_err(|e| {
        Failure::new(format!(
            "cannot use {} for the private launch file: {e}",
            dir.display()
        ))
    })?;
    let mut tag = [0u8; 6];
    getrandom::fill(&mut tag).expect("operating system random source is available");
    let tag = hex::encode(tag);
    let path = dir.join(format!("admin-{}-{tag}.html", std::process::id()));
    crate::instance::write_private(&path, &page(url))
        .map_err(|e| Failure::new(format!("cannot write {}: {e}", path.display())))?;
    Ok(LaunchFile(path))
}

pub fn write_url_file(path: &Path, url: &str) -> Result<(), Failure> {
    crate::instance::write_private(path, &format!("{url}\n"))
        .map_err(|e| Failure::new(format!("cannot write {}: {e}", path.display())))
}

pub fn open_browser(file: &Path) -> bool {
    let target = file.as_os_str();
    let spawned = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    } else {
        std::process::Command::new("xdg-open")
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    spawned.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launch_file_is_private_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let url = "http://127.0.0.1:4000/#token=abc\"<x>";
        let f = write(dir.path(), url).unwrap();
        let text = std::fs::read_to_string(f.path()).unwrap();
        assert!(text.contains("url=http://127.0.0.1:4000/#token=abc&quot;&lt;x&gt;"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(f.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let path = f.path().to_path_buf();
        drop(f);
        assert!(!path.exists());
    }
}
