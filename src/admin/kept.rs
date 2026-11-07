use super::core::ports::Version;
use super::core::text;

#[cfg(unix)]
pub(super) fn names_at(fd: &rustix::fd::OwnedFd) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in rustix::fs::Dir::read_from(fd)? {
        out.push(entry?.file_name().to_string_lossy().into_owned());
    }
    Ok(out)
}

#[cfg(unix)]
pub(super) fn kept_at(fd: &rustix::fd::OwnedFd) -> std::io::Result<Vec<(u128, Version, String)>> {
    let mut out: Vec<(u128, Version, String)> = names_at(fd)?
        .into_iter()
        .filter_map(|name| {
            let (nanos, v) = parse_kept(&name)?;
            Some((nanos, v, name))
        })
        .collect();
    out.sort_by_key(|(n, _, _)| std::cmp::Reverse(*n));
    Ok(out)
}

#[cfg(unix)]
pub(super) fn put_at(
    fd: &rustix::fd::OwnedFd,
    name: &str,
    body: &str,
    owner: Option<(u32, u32)>,
) -> std::io::Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags};
    let draft = format!(".{name}.burrow-new-{}", std::process::id());
    let _ = rustix::fs::unlinkat(fd, draft.as_str(), AtFlags::empty());
    let written = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::from(rustix::fs::openat(
            fd,
            draft.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )?);
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        if let Some(owner) = owner {
            let _ = super::adapters::hand_over(&file, owner);
        }
        rustix::fs::renameat(fd, draft.as_str(), fd, name)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = rustix::fs::unlinkat(fd, draft.as_str(), AtFlags::empty());
    }
    written
}

pub(super) fn parse_kept(name: &str) -> Option<(u128, Version)> {
    let (at, rest) = name.split_once('-')?;
    let hex = rest.split('.').next()?;
    let nanos: u128 = at.parse().ok()?;
    let rev = text::rev_from_hex(hex)?;
    Some((
        nanos,
        Version {
            rev,
            at: (nanos / 1_000_000_000) as u64,
        },
    ))
}
