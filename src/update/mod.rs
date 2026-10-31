pub mod archive;
pub mod http;

use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::base::failure::{Failure, admin, cmd};
use crate::base::output::say;
use crate::install::Manifest;
use crate::service::{self, parse_version};

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
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else if cfg!(windows) {
        "windows"
    } else {
        return None;
    };
    let arch = if cfg!(target_arch = "x86_64") {
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
        return None;
    };
    let name = match (os, arch) {
        ("linux", "amd64") => "linux-amd64",
        ("linux", "arm64") => "linux-arm64",
        ("linux", "armv7") => "linux-armv7",
        ("linux", "armv6") => "linux-armv6",
        ("linux", "riscv64") => "linux-riscv64",
        ("macos", "amd64") => "macos-amd64",
        ("macos", "arm64") => "macos-arm64",
        ("freebsd", "amd64") => "freebsd-amd64",
        ("windows", "amd64") => "windows-amd64",
        ("windows", "arm64") => "windows-arm64",
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

fn in_container() -> bool {
    cfg!(target_os = "linux")
        && (Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists())
}

fn fetch(url: &str, limit: usize) -> Result<http::Fetched, http::FetchError> {
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
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.dir).is_ok() || !self.dir.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
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

fn version_of(v: &str) -> Result<semver::Version, Failure> {
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
    let (sums_url, sums) = fetch_sums(&base, wanted.as_deref())?;
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
        say_available(current, wanted.as_deref(), &asset.version, mine, out)?;
        return Ok(0);
    }
    if let Some(line) = current.filter(|_| !opts.force) {
        already_current(&line, &theirs, &ours, mine, out)?;
        return Ok(0);
    }

    let me = service::this_binary()?;
    let installed = crate::install::all_manifests();
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

    let staged = download(&base, &asset, ext, &me, &theirs, out)?;

    let targets = Targets {
        managed: &managed,
        me: &me,
        me_is_service,
        admin_rights,
    };
    let force = opts.force || theirs < ours;
    finish(&staged, &targets, force, &asset.version, mine, out)
}

fn fetch_sums(base: &str, wanted: Option<&str>) -> Result<(String, String), Failure> {
    let sums_url = match wanted {
        Some(v) => format!("{base}/download/v{v}/SHA256SUMS"),
        None => format!("{base}/latest/download/SHA256SUMS"),
    };
    let sums = match fetch(&sums_url, MAX_SUMS) {
        Ok(http::Fetched::Body(b)) => String::from_utf8_lossy(&b).into_owned(),
        Ok(http::Fetched::NotFound) => {
            let what = match wanted {
                Some(v) => format!("burrow {v} is not released"),
                None => "no burrow release was found".to_string(),
            };
            return Err(Failure::new(format!("{what} at {base}"))
                .hint(format!("see the releases at {base}")));
        }
        Err(e @ http::FetchError::Insecure(_)) => {
            return Err(Failure::new(format!("cannot check for a new version: {e}")));
        }
        Err(e) => {
            return Err(Failure::new(format!("cannot check for a new version: {e}"))
                .hint(connect_hint(&sums_url, &|name| std::env::var(name).ok())));
        }
    };
    Ok((sums_url, sums))
}

fn say_available(
    current: Option<String>,
    wanted: Option<&str>,
    version: &str,
    mine: &str,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let line = current.unwrap_or_else(|| {
        let how = match wanted {
            Some(v) => cmd(&format!("update --to {v}")),
            None => cmd("update"),
        };
        format!(
            "burrow {} is available (this is {mine}); install it with: {how}",
            version
        )
    });
    say(out, line)?;
    Ok(())
}

fn already_current(
    line: &str,
    theirs: &semver::Version,
    ours: &semver::Version,
    mine: &str,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let me = service::this_binary()?;
    let installed = crate::install::all_manifests();
    let behind: Vec<&Manifest> = installed
        .iter()
        .filter(|m| m.copied_binary && m.binary != me)
        .filter(|m| version_of(&service::recorded_version(m)).is_ok_and(|v| v < *ours))
        .collect();
    for m in installed.iter().filter(|m| service::exposed(m)) {
        say(out, format!("{}: {}", m.name, service::exposed_note(m)))?;
    }
    if behind.is_empty() {
        say(out, format!("{line}, nothing to do"))?;
    } else if theirs == ours && service::is_admin() {
        say(
            out,
            format!("{line}; the installed services run an older version"),
        )?;
        service::update_from(&me, mine, None, false, out)?;
    } else {
        say(
            out,
            format!(
                "{line}, but the installed services run the older {}; update them with: {}",
                service::recorded_version(behind[0]),
                update_services_with(&me)
            ),
        )?;
    }
    Ok(())
}

fn download(
    base: &str,
    asset: &Asset,
    ext: &str,
    me: &Path,
    theirs: &semver::Version,
    out: &mut dyn Write,
) -> Result<Staged, Failure> {
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
    let got = hex::encode(Sha256::digest(&data));
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
    let staged = stage(me, &binary)?;
    let reported = service::binary_version(&staged.binary);
    if parse_version(&reported).as_ref() != Some(theirs) {
        return Err(Failure::new(format!(
            "the downloaded binary reports \"{reported}\" instead of {}; nothing was changed",
            asset.version
        )));
    }
    say(
        out,
        format!("verified {} (SHA-256 {})", asset.name, &asset.sha256[..16]),
    )?;
    Ok(staged)
}

struct Targets<'a> {
    managed: &'a [&'a Manifest],
    me: &'a Path,
    me_is_service: bool,
    admin_rights: bool,
}

fn finish(
    staged: &Staged,
    t: &Targets,
    force: bool,
    version: &str,
    mine: &str,
    out: &mut dyn Write,
) -> Result<u8, Failure> {
    let (managed, me, me_is_service, admin_rights) =
        (t.managed, t.me, t.me_is_service, t.admin_rights);
    if !managed.is_empty() && admin_rights {
        service::update_from(&staged.binary, version, None, force, out)?;
    }
    if !me_is_service {
        service::copy_file(&staged.binary, me, true).map_err(|f| {
            f.hint(format!(
                "if you lack the rights to change it, run: {}",
                admin("update")
            ))
        })?;
        say(
            out,
            format!("updated {} {mine} -> {}", me.display(), version),
        )?;
    }
    if managed.is_empty() {
        service::note_strays(out)?;
    }
    if !managed.is_empty() && !admin_rights {
        say(
            out,
            format!(
                "the installed services still use {}; update them with: {}",
                managed[0].binary.display(),
                update_services_with(me)
            ),
        )?;
    }
    Ok(0)
}

fn update_services_with(me: &Path) -> String {
    let me = crate::base::failure::path_arg(me);
    if cfg!(windows) {
        format!("{me} service update (as administrator)")
    } else {
        format!("sudo {me} service update")
    }
}

fn connect_hint(url: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let proxy = http::Url::parse(url)
        .ok()
        .and_then(|u| http::proxy_from_env(&u, env).ok().flatten());
    let mut hint = match proxy {
        Some(p) => format!("check the internet connection and the proxy {p:?}"),
        None => proxy_hint(env("SUDO_USER").is_some()),
    };
    if let Some(custom) = env("BURROW_UPDATE_URL").filter(|v| !v.trim().is_empty()) {
        hint.push_str(&format!(
            "; BURROW_UPDATE_URL is set to {}, unset it to use the official releases",
            custom.trim()
        ));
    }
    hint
}

fn proxy_hint(under_sudo: bool) -> String {
    let how = if cfg!(windows) {
        "set $env:HTTPS_PROXY = \"http://host:port\"".to_string()
    } else if under_sudo {
        format!(
            "set HTTPS_PROXY=http://host:port, and keep it through sudo with: sudo --preserve-env=HTTPS_PROXY,NO_PROXY {} update",
            crate::base::failure::command()
        )
    } else {
        "set HTTPS_PROXY=http://host:port".to_string()
    };
    format!("check the internet connection; behind a proxy, {how}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_connect_hint_names_the_proxy_and_the_update_url_in_use() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v.to_string())
            }
        };
        let url = "https://github.com/x/y/releases/latest/download/SHA256SUMS";
        let plain = connect_hint(url, &env(&[]));
        assert!(plain.contains("behind a proxy, set"), "{plain}");
        assert!(!plain.contains("BURROW_UPDATE_URL"), "{plain}");
        let proxied = connect_hint(url, &env(&[("HTTPS_PROXY", "http://proxy:3128")]));
        assert!(proxied.contains("the proxy http://proxy:3128"), "{proxied}");
        assert!(!proxied.contains("behind a proxy, set"), "{proxied}");
        let custom = connect_hint(
            "http://mirror.lan/burrow/latest/download/SHA256SUMS",
            &env(&[
                ("HTTPS_PROXY", "http://proxy:3128"),
                ("BURROW_UPDATE_URL", "http://mirror.lan/burrow"),
            ]),
        );
        assert!(custom.contains("behind a proxy, set"), "{custom}");
        assert!(
            custom.contains("BURROW_UPDATE_URL is set to http://mirror.lan/burrow"),
            "{custom}"
        );
    }

    #[test]
    fn the_proxy_hint_says_how_to_keep_the_variable_through_sudo() {
        let plain = proxy_hint(false);
        assert!(plain.contains("HTTPS_PROXY"), "{plain}");
        assert!(!plain.contains("--preserve-env"), "{plain}");
        if cfg!(unix) {
            assert!(proxy_hint(true).contains("sudo --preserve-env=HTTPS_PROXY,NO_PROXY"));
        }
    }

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
    fn each_release_build_updates_from_its_own_asset() {
        let workflow = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.github/workflows/release.yml"
        ))
        .unwrap();
        let mut rows = 0;
        for line in workflow.lines() {
            let field = |key: &str| {
                line.split(&format!("{key}: "))
                    .nth(1)
                    .and_then(|rest| rest.split([',', ' ']).next())
            };
            let (Some(asset), Some(target)) = (field("asset"), field("target")) else {
                continue;
            };
            rows += 1;
            if target == env!("BURROW_TARGET") {
                assert_eq!(platform().map(|(name, _)| name), Some(asset));
            }
        }
        assert_eq!(rows, 10);
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
            hex::encode(Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
