pub fn os() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else if cfg!(windows) {
        "windows"
    } else {
        std::env::consts::OS
    }
}

pub fn arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "arm") {
        if env!("BURROW_TARGET").starts_with("armv7") {
            "armv7"
        } else {
            "armv6"
        }
    } else if cfg!(target_arch = "riscv64") {
        "riscv64"
    } else {
        std::env::consts::ARCH
    }
}

pub const ASSETS: [&str; 10] = [
    "linux-amd64",
    "linux-arm64",
    "linux-armv7",
    "linux-armv6",
    "linux-riscv64",
    "macos-amd64",
    "macos-arm64",
    "freebsd-amd64",
    "windows-amd64",
    "windows-arm64",
];

pub fn release_asset() -> Option<(&'static str, &'static str)> {
    let wanted = format!("{}-{}", os(), arch());
    let name = ASSETS.into_iter().find(|a| *a == wanted)?;
    Some((name, if cfg!(windows) { "zip" } else { "tar.gz" }))
}

pub fn version() -> &'static str {
    test_mark("VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub const TEST_MARK: &str = "BURROW-TEST-";

pub fn test_break_at_start() -> Option<crate::base::failure::Failure> {
    use crate::base::failure::{EXIT_CONFIG, Failure};
    match test_mark("BREAK")? {
        "start" => Some(Failure::new("this test build stops at once")),
        "config" => {
            let mut f = Failure::new("this test build accepts no config");
            f.exit = EXIT_CONFIG;
            Some(f)
        }
        _ => None,
    }
}

#[cfg(feature = "admin-test-hooks")]
pub fn test_mark(key: &str) -> Option<&'static str> {
    use std::io::{Read, Seek, SeekFrom};
    static MARKS: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    let marks = MARKS.get_or_init(|| {
        let mut tail = Vec::new();
        let read = std::env::current_exe()
            .and_then(std::fs::File::open)
            .and_then(|mut f| {
                let len = f.metadata()?.len();
                f.seek(SeekFrom::Start(len.saturating_sub(256)))?;
                f.read_to_end(&mut tail)
            });
        if read.is_err() {
            return Vec::new();
        }
        String::from_utf8_lossy(&tail)
            .lines()
            .filter_map(|l| l.strip_prefix(TEST_MARK)?.split_once('='))
            .map(|(k, v)| (k.to_string(), v.trim().to_string()))
            .collect()
    });
    marks
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

#[cfg(not(feature = "admin-test-hooks"))]
pub fn test_mark(_key: &str) -> Option<&'static str> {
    None
}
