use burrow_admin_proto::plan::{Download, Downloads};

use crate::base::build::ASSETS;

fn released(version: &str) -> Option<semver::Version> {
    let v = semver::Version::parse(version.trim().trim_start_matches('v')).ok()?;
    let dev = v.pre.as_str().starts_with("dev") || !v.build.is_empty();
    (!dev).then_some(v)
}

fn label(os: &str, arch: &str) -> String {
    match (os, arch) {
        ("macos", "arm64") => "Apple silicon (arm64)".into(),
        ("macos", "amd64") => "Intel (amd64)".into(),
        ("linux", "armv7") => "armv7 (Raspberry Pi 2/3)".into(),
        ("linux", "armv6") => "armv6 (Pi Zero/1)".into(),
        _ => arch.to_string(),
    }
}

fn preferred(os: &str, arch: &str) -> bool {
    arch == if os == "macos" { "arm64" } else { "amd64" }
}

pub fn list(version: &str, releases: &str) -> Option<Downloads> {
    let v = released(version)?;
    let base = releases.trim_end_matches('/');
    let files = ASSETS
        .iter()
        .filter_map(|asset| {
            let (os, arch) = asset.split_once('-')?;
            let ext = if os == "windows" { "zip" } else { "tar.gz" };
            let folder = format!("burrow-{v}-{asset}");
            Some(Download {
                os: os.to_string(),
                arch: arch.to_string(),
                label: label(os, arch),
                url: format!("{base}/download/v{v}/{folder}.{ext}"),
                folder,
                preferred: preferred(os, arch),
            })
        })
        .collect();
    Some(Downloads {
        version: v.to_string(),
        page: format!("{base}/tag/v{v}"),
        files,
    })
}

pub fn current() -> Option<Downloads> {
    list(crate::base::build::version(), &crate::update::releases())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GH: &str = "https://github.com/o/burrow/releases";

    #[test]
    fn every_release_asset_is_listed_with_its_archive_and_folder() {
        let d = list("0.2.9", GH).expect("a release");
        assert_eq!(d.version, "0.2.9");
        assert_eq!(d.page, format!("{GH}/tag/v0.2.9"));
        assert_eq!(d.files.len(), ASSETS.len());
        for (f, asset) in d.files.iter().zip(ASSETS) {
            assert_eq!(format!("{}-{}", f.os, f.arch), asset);
            assert_eq!(f.folder, format!("burrow-0.2.9-{asset}"));
            let ext = if f.os == "windows" { "zip" } else { "tar.gz" };
            assert_eq!(
                f.url,
                format!("{GH}/download/v0.2.9/burrow-0.2.9-{asset}.{ext}")
            );
        }
        let linux = &d.files[0];
        assert_eq!(
            linux.url,
            "https://github.com/o/burrow/releases/download/v0.2.9/burrow-0.2.9-linux-amd64.tar.gz"
        );
    }

    #[test]
    fn one_preferred_chip_per_system_with_readable_labels() {
        let d = list("v1.4.0", &format!("{GH}/")).expect("a release");
        let pick = |os: &str| -> Vec<&str> {
            d.files
                .iter()
                .filter(|f| f.os == os && f.preferred)
                .map(|f| f.arch.as_str())
                .collect()
        };
        assert_eq!(pick("linux"), ["amd64"]);
        assert_eq!(pick("macos"), ["arm64"]);
        assert_eq!(pick("windows"), ["amd64"]);
        assert_eq!(pick("freebsd"), ["amd64"]);
        let label = |a: &str| {
            d.files
                .iter()
                .find(|f| f.folder.ends_with(a))
                .map(|f| f.label.clone())
        };
        assert_eq!(
            label("macos-arm64").as_deref(),
            Some("Apple silicon (arm64)")
        );
        assert_eq!(label("macos-amd64").as_deref(), Some("Intel (amd64)"));
        assert_eq!(
            label("linux-armv7").as_deref(),
            Some("armv7 (Raspberry Pi 2/3)")
        );
        assert_eq!(label("linux-armv6").as_deref(), Some("armv6 (Pi Zero/1)"));
        assert_eq!(label("linux-riscv64").as_deref(), Some("riscv64"));
        assert_eq!(d.page, format!("{GH}/tag/v1.4.0"));
        assert!(
            d.files[0]
                .url
                .starts_with(&format!("{GH}/download/v1.4.0/"))
        );
    }

    #[test]
    fn a_build_without_a_release_offers_no_download() {
        for v in [
            "0.0.0-dev.abc1234",
            "0.2.9-dev",
            "0.2.9+local",
            "not a version",
            "",
        ] {
            assert_eq!(list(v, GH), None, "{v}");
        }
        let rc = list("0.3.0-rc.1", GH).expect("a prerelease is released");
        assert!(
            rc.files[0]
                .url
                .ends_with("/v0.3.0-rc.1/burrow-0.3.0-rc.1-linux-amd64.tar.gz")
        );
    }

    #[test]
    fn the_running_version_and_release_url_are_used() {
        let d = current().expect("the crate version is a release");
        assert_eq!(d.version, env!("CARGO_PKG_VERSION"));
        assert!(d.page.starts_with(&crate::update::releases()));
    }
}
