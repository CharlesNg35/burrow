use std::path::{Path, PathBuf};
use std::process::Command;

pub const OLDER: &str = "0.1.2";
const RELEASES: &str = "https://github.com/CharlesNg35/burrow/releases/download";
const ASSETS: &[(&str, &str, &str)] = &[
    (
        "linux-amd64",
        "tar.gz",
        "86dcfd460f25c1e9d7024f538487323a4491d7bbea28ee65029faba2719b55dd",
    ),
    (
        "linux-arm64",
        "tar.gz",
        "d06ff6a7dc2cc262ea7039df096beef349ceb342267794b9dcb9303b69336a6e",
    ),
    (
        "macos-amd64",
        "tar.gz",
        "a71b1c5ec179f99b98ca65687acbf32b70d533ad674b69da262a1fd50b41e42a",
    ),
    (
        "macos-arm64",
        "tar.gz",
        "606bbf621e798004b676b6bc6e83d06fd522e4003834597b818df6bf604c7ba5",
    ),
    (
        "windows-amd64",
        "zip",
        "96f32cf421ccd4a6f6d1b70b7b512c6d22cc4b45c3eeda7c538359a17cec2e86",
    ),
    (
        "windows-arm64",
        "zip",
        "41eb759b2385b1106770508ffc6358073dd4759568b54289417125f647a1bf3b",
    ),
];

fn platform() -> Option<&'static str> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        "windows" => "windows",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => return None,
    };
    Some(match (os, arch) {
        ("linux", "amd64") => "linux-amd64",
        ("linux", _) => "linux-arm64",
        ("macos", "amd64") => "macos-amd64",
        ("macos", _) => "macos-arm64",
        (_, "amd64") => "windows-amd64",
        _ => "windows-arm64",
    })
}

pub fn older_binary(root: &Path) -> Result<PathBuf, String> {
    let platform = platform().ok_or("no burrow release is pinned for this platform")?;
    let (_, ext, sha) = ASSETS
        .iter()
        .find(|(p, _, _)| *p == platform)
        .ok_or("no burrow release is pinned for this platform")?;
    let name = format!("burrow-{OLDER}-{platform}");
    let home = root.join(".tools").join(&name);
    let exe = if cfg!(windows) {
        "burrow.exe"
    } else {
        "burrow"
    };
    let bin = home.join(&name).join(exe);
    if bin.exists() {
        return Ok(bin);
    }
    std::fs::create_dir_all(&home).map_err(|e| format!("{}: {e}", home.display()))?;
    let archive = home.join(format!("{name}.{ext}"));
    crate::tools::fetch(&format!("{RELEASES}/v{OLDER}/{name}.{ext}"), &archive, sha)?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&home)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    let _ = std::fs::remove_file(&archive);
    if !status.success() || !bin.exists() {
        return Err(format!("unpacking {name} failed"));
    }
    Ok(bin)
}

fn cargo_test(root: &Path, bin: &Path, args: &[&str]) -> Result<bool, String> {
    Command::new("cargo")
        .args(["test", "--locked", "-p", "burrow"])
        .args(args)
        .env("BURROW_COMPAT_BIN", bin)
        .env("BURROW_E2E_REQUIRE", "1")
        .current_dir(root)
        .status()
        .map(|s| s.success())
        .map_err(|e| format!("cargo: {e}"))
}

pub fn run(root: &Path) -> Result<(), String> {
    let bin = older_binary(root)?;
    println!("burrow {OLDER}: {}", bin.display());
    if !cargo_test(root, &bin, &["--test", "compat"])? {
        return Err(format!(
            "the compatibility tests against burrow {OLDER} failed"
        ));
    }
    println!(
        "the admin UI both ways against burrow {OLDER}, in the pinned Chrome (needs cargo xtask tools and cargo xtask ui)"
    );
    let ui = ["--test", "admin_e2e", "compat::", "--", "--test-threads=1"];
    if !cargo_test(root, &bin, &ui)? {
        return Err(format!(
            "the admin UI compatibility flows against burrow {OLDER} failed"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pinned_asset_has_a_full_sha256() {
        for (p, ext, sha) in ASSETS {
            assert!(
                sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
                "{p}"
            );
            assert_eq!(*ext == "zip", p.starts_with("windows"), "{p}");
        }
        assert!(platform().is_none_or(|p| ASSETS.iter().any(|(a, _, _)| *a == p)));
    }
}
