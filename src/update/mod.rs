pub mod archive;
pub mod http;

use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::failure::{Failure, admin, cmd};
use crate::service::{self, Manifest, Version, parse_version};

const MAX_SUMS: usize = 1024 * 1024;
const MAX_ASSET: usize = 256 * 1024 * 1024;

pub struct Options {
    pub check: bool,
    pub to: Option<String>,
    pub force: bool,
}

pub fn releases() -> String {
    std::env::var("BURROW_UPDATE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| concat!(env!("CARGO_PKG_REPOSITORY"), "/releases").to_string())
        .trim_end_matches('/')
        .to_string()
}

pub fn platform() -> Option<(&'static str, &'static str)> {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        return None;
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        return None;
    };
    let name = match (os, arch) {
        ("linux", "amd64") => "linux-amd64",
        ("linux", "arm64") => "linux-arm64",
        ("macos", "amd64") => "macos-amd64",
        ("macos", "arm64") => "macos-arm64",
        ("windows", "amd64") => "windows-amd64",
        _ => return None,
    };
    Some((name, if os == "windows" { "zip" } else { "tar.gz" }))
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "burrow.exe"
    } else {
        "burrow"
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Asset {
    pub version: String,
    pub name: String,
    pub sha256: String,
}

pub fn find_asset(sums: &str, platform: &str, ext: &str) -> Option<Asset> {
    let suffix = format!("-{platform}.{ext}");
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        let version = name.strip_prefix("burrow-")?.strip_suffix(&suffix)?;
        parse_version(version)?;
        (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| Asset {
            version: version.to_string(),
            name: name.to_string(),
            sha256: hash.to_ascii_lowercase(),
        })
    })
}

fn say(out: &mut dyn Write, line: impl AsRef<str>) -> Result<(), Failure> {
    match writeln!(out, "{}", line.as_ref()).and_then(|_| out.flush()) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => {
            Err(Failure::new(format!("cannot write output: {e}")))
        }
        _ => Ok(()),
    }
}

fn in_container() -> bool {
    cfg!(target_os = "linux")
        && (Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists())
}

fn fetch(url: &str, limit: usize) -> Result<http::Fetched, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the async runtime: {e}"))?;
    let env = |name: &str| std::env::var(name).ok();
    rt.block_on(http::get(url, limit, &env))
}

struct Staged {
    dir: PathBuf,
    binary: PathBuf,
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o755);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

const STAGED_PREFIX: &str = ".burrow-update-";
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(600);

fn sweep_stale(parent: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE_AFTER);
        if stale
            && entry
                .file_name()
                .to_string_lossy()
                .starts_with(STAGED_PREFIX)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn stage(near: &Path, bytes: &[u8]) -> Result<Staged, Failure> {
    let folder = format!("{STAGED_PREFIX}{}", std::process::id());
    let places = [
        near.parent().map(|p| p.join(&folder)),
        Some(std::env::temp_dir().join(&folder)),
    ];
    let mut last = None;
    for dir in places.into_iter().flatten() {
        if let Some(parent) = dir.parent() {
            sweep_stale(parent);
        }
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(e) = std::fs::create_dir(&dir) {
            last = Some(e);
            continue;
        }
        let staged = Staged {
            binary: dir.join(binary_name()),
            dir,
        };
        match write_executable(&staged.binary, bytes) {
            Ok(()) => return Ok(staged),
            Err(e) => last = Some(e),
        }
    }
    Err(Failure::new(format!(
        "cannot save the downloaded binary: {}",
        last.map_or_else(|| "no writable folder".to_string(), |e| e.to_string())
    )))
}

fn version_of(v: &str) -> Result<Version, Failure> {
    parse_version(v).ok_or_else(|| Failure::usage(format!("\"{v}\" is not a version like 1.2.3")))
}

pub fn run(opts: &Options, out: &mut dyn Write) -> Result<u8, Failure> {
    if in_container() {
        return Err(
            Failure::new("burrow runs in a container here; update the image instead").hint(
                "pull the new image, for example: docker pull ghcr.io/charlesng35/burrow:latest",
            ),
        );
    }
    let (platform, ext) = platform().ok_or_else(|| {
        Failure::new("there is no release build of burrow for this system")
            .hint("build the new version from source: cargo install --git https://github.com/charlesng35/burrow")
    })?;
    let mine = env!("CARGO_PKG_VERSION");
    let base = releases();
    let wanted = match &opts.to {
        Some(v) => {
            let v = v.trim().trim_start_matches('v');
            version_of(v)?;
            Some(v.to_string())
        }
        None => None,
    };
    let sums_url = match &wanted {
        Some(v) => format!("{base}/download/v{v}/SHA256SUMS"),
        None => format!("{base}/latest/download/SHA256SUMS"),
    };
    let sums = match fetch(&sums_url, MAX_SUMS) {
        Ok(http::Fetched::Body(b)) => String::from_utf8_lossy(&b).into_owned(),
        Ok(http::Fetched::NotFound) => {
            let what = match &wanted {
                Some(v) => format!("burrow {v} is not released"),
                None => "no burrow release was found".to_string(),
            };
            return Err(Failure::new(format!("{what} at {base}"))
                .hint(format!("see the releases at {base}")));
        }
        Err(e) => {
            return Err(Failure::new(format!("cannot check for a new version: {e}")).hint(
                "check the internet connection; behind a proxy, set HTTPS_PROXY=http://host:port",
            ));
        }
    };
    let asset = find_asset(&sums, platform, ext).ok_or_else(|| {
        Failure::new(format!(
            "the release has no {platform} build (looked in {sums_url})"
        ))
    })?;
    let theirs = version_of(&asset.version)?;
    let ours = version_of(mine)?;
    let current = match theirs.cmp(&ours) {
        std::cmp::Ordering::Equal => Some(format!("burrow {mine} is up to date")),
        std::cmp::Ordering::Less if wanted.is_none() => Some(format!(
            "this burrow ({mine}) is newer than the latest release ({})",
            asset.version
        )),
        _ => None,
    };
    if opts.check {
        let line = current.unwrap_or_else(|| {
            let how = match &wanted {
                Some(v) => cmd(&format!("update --to {v}")),
                None => cmd("update"),
            };
            format!(
                "burrow {} is available (this is {mine}); install it with: {how}",
                asset.version
            )
        });
        say(out, line)?;
        return Ok(0);
    }
    if let Some(line) = current.filter(|_| !opts.force) {
        say(out, format!("{line}, nothing to do"))?;
        return Ok(0);
    }

    let me = service::this_binary()?;
    let installed = service::all_manifests();
    let managed: Vec<&Manifest> = installed.iter().filter(|m| m.copied_binary).collect();
    let foreign = installed.iter().any(|m| !m.copied_binary && m.binary == me);
    let me_is_service = managed.iter().any(|m| m.binary == me);
    let admin_rights = service::is_admin();
    if me_is_service && !admin_rights {
        return Err(Failure::new(format!(
            "{} is the binary of the installed services; replacing it needs administrator rights",
            me.display()
        ))
        .hint(format!("run: {}", admin("update"))));
    }
    if foreign {
        return Err(Failure::new(format!(
            "{} is not managed by burrow; upgrade it where you installed it",
            me.display()
        )));
    }

    say(out, format!("downloading {}", asset.name))?;
    let url = format!("{base}/download/v{}/{}", asset.version, asset.name);
    let data = match fetch(&url, MAX_ASSET) {
        Ok(http::Fetched::Body(b)) => b,
        Ok(http::Fetched::NotFound) => {
            return Err(Failure::new(format!(
                "{url} is listed in SHA256SUMS but does not exist"
            )));
        }
        Err(e) => return Err(Failure::new(format!("cannot download {}: {e}", asset.name))),
    };
    let got = hex(&Sha256::digest(&data));
    if got != asset.sha256 {
        return Err(Failure::new(format!(
            "{} does not match its SHA-256 in SHA256SUMS (expected {}, got {got}); nothing was changed",
            asset.name, asset.sha256
        ))
        .hint("try again; if it keeps failing, report it at https://github.com/charlesng35/burrow/issues"));
    }
    let binary = match ext {
        "zip" => archive::from_zip(&data, binary_name()),
        _ => archive::from_tar_gz(&data, binary_name()),
    }
    .map_err(|e| Failure::new(format!("cannot unpack {}: {e}", asset.name)))?;
    let staged = stage(&me, &binary)?;
    let reported = service::binary_version(&staged.binary);
    if parse_version(&reported).as_ref() != Some(&theirs) {
        return Err(Failure::new(format!(
            "the downloaded binary reports \"{reported}\" instead of {}; nothing was changed",
            asset.version
        )));
    }
    say(
        out,
        format!("verified {} (SHA-256 {})", asset.name, &asset.sha256[..16]),
    )?;

    if !managed.is_empty() && admin_rights {
        let force = opts.force || theirs < ours;
        service::update_from(&staged.binary, &asset.version, None, force, out)?;
    }
    if !me_is_service {
        service::copy_file(&staged.binary, &me, true).map_err(|f| {
            f.hint(format!(
                "if you lack the rights to change it, run: {}",
                admin("update")
            ))
        })?;
        say(
            out,
            format!("updated {} {mine} -> {}", me.display(), asset.version),
        )?;
    }
    if !managed.is_empty() && !admin_rights {
        say(
            out,
            format!(
                "the installed services still use {}; update them with: {}",
                managed[0].binary.display(),
                admin("service update")
            ),
        )?;
    }
    Ok(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMS: &str = "\
1111111111111111111111111111111111111111111111111111111111111111  burrow-0.3.0-linux-amd64.tar.gz
2222222222222222222222222222222222222222222222222222222222222222  burrow-0.3.0-windows-amd64.zip
3333333333333333333333333333333333333333333333333333333333333333 *burrow-0.3.0-macos-arm64.tar.gz
";

    #[test]
    fn picks_the_asset_for_this_platform() {
        let a = find_asset(SUMS, "linux-amd64", "tar.gz").unwrap();
        assert_eq!(
            a,
            Asset {
                version: "0.3.0".into(),
                name: "burrow-0.3.0-linux-amd64.tar.gz".into(),
                sha256: "1".repeat(64),
            }
        );
        assert_eq!(
            find_asset(SUMS, "windows-amd64", "zip").unwrap().sha256,
            "2".repeat(64)
        );
        assert_eq!(
            find_asset(SUMS, "macos-arm64", "tar.gz").unwrap().name,
            "burrow-0.3.0-macos-arm64.tar.gz"
        );
        assert!(find_asset(SUMS, "linux-arm64", "tar.gz").is_none());
        assert!(
            find_asset(
                "xyz  burrow-0.3.0-linux-amd64.tar.gz",
                "linux-amd64",
                "tar.gz"
            )
            .is_none()
        );
        assert!(
            find_asset(
                &format!("{}  burrow-latest-linux-amd64.tar.gz", "a".repeat(64)),
                "linux-amd64",
                "tar.gz"
            )
            .is_none()
        );
    }

    #[test]
    fn this_platform_has_a_release_build() {
        if cfg!(all(
            any(target_os = "linux", target_os = "macos"),
            any(target_arch = "x86_64", target_arch = "aarch64")
        )) {
            assert!(platform().is_some());
        }
    }

    #[cfg(unix)]
    #[test]
    fn only_old_download_folders_are_swept() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join(".burrow-update-1");
        let fresh = root.path().join(".burrow-update-2");
        let other = root.path().join("keep");
        for d in [&old, &fresh, &other] {
            std::fs::create_dir(d).unwrap();
        }
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for d in [&old, &other] {
            std::fs::File::open(d)
                .unwrap()
                .set_modified(hour_ago)
                .unwrap();
        }
        sweep_stale(root.path());
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(other.exists());
    }

    #[test]
    fn hashes_as_lowercase_hex() {
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
