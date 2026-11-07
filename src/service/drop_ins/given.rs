use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::Path;

use rustix::fs::{AtFlags, FileType, Mode, OFlags, Stat};

use crate::config::docs::drop_in_folder;
use crate::service::nofollow::plain_files;

const GIVEN: &str = ".burrow-given";
const FOLDER: &str = ".";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Parts {
    owner: bool,
    group: bool,
}

impl Parts {
    fn of(st: &Stat, (uid, gid): (u32, u32)) -> Parts {
        Parts {
            owner: uid != 0 && st.st_uid == uid,
            group: gid != 0 && st.st_gid == gid,
        }
    }

    fn any(self) -> bool {
        self.owner || self.group
    }

    fn give(self, f: &OwnedFd, (uid, gid): (u32, u32)) -> bool {
        let (uid, gid) = (self.owner.then_some(uid), self.group.then_some(gid));
        std::os::unix::fs::fchown(f, uid, gid).is_ok()
    }
}

type Given = Vec<(Parts, String)>;

fn line((parts, name): &(Parts, String)) -> String {
    let owner = if parts.owner { 'u' } else { '-' };
    let group = if parts.group { 'g' } else { '-' };
    format!("{owner}{group} {name}\n")
}

fn parse(line: &str) -> Option<(Parts, String)> {
    let (flags, name) = line.split_once(' ')?;
    let parts = match flags {
        "u-" => Parts {
            owner: true,
            group: false,
        },
        "-g" => Parts {
            owner: false,
            group: true,
        },
        "ug" => Parts {
            owner: true,
            group: true,
        },
        _ => return None,
    };
    (!name.is_empty() && name != ".." && !name.contains('/')).then(|| (parts, name.to_string()))
}

fn opened(dir: &OwnedFd, name: &str) -> Option<(OwnedFd, Stat)> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let f = rustix::fs::openat(dir, name, flags, Mode::empty()).ok()?;
    let st = rustix::fs::fstat(&f).ok()?;
    let kind = FileType::from_raw_mode(st.st_mode);
    let plain = kind == FileType::RegularFile && st.st_nlink == 1;
    let fits = if name == FOLDER {
        kind == FileType::Directory
    } else {
        plain
    };
    fits.then_some((f, st))
}

fn read(dir: &OwnedFd) -> Given {
    let mut text = String::new();
    let trusted = opened(dir, GIVEN)
        .filter(|(_, st)| st.st_uid == 0 && st.st_mode & 0o022 == 0)
        .map(|(f, _)| std::fs::File::from(f).read_to_string(&mut text));
    match trusted {
        Some(Ok(_)) => text.lines().filter_map(parse).collect(),
        _ => Vec::new(),
    }
}

fn write(dir: &OwnedFd, given: &Given) {
    let temp = format!("{GIVEN}.burrow-new");
    let _ = rustix::fs::unlinkat(dir, temp.as_str(), AtFlags::empty());
    let text: String = given.iter().map(line).collect();
    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::from(rustix::fs::openat(
            dir,
            temp.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )?);
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::os::unix::fs::fchown(&f, Some(0), Some(0))?;
        rustix::fs::renameat(dir, temp.as_str(), dir, GIVEN)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = rustix::fs::unlinkat(dir, temp.as_str(), AtFlags::empty());
    }
}

fn move_parts(folder: &Path, from: (u32, u32), to: (u32, u32)) -> Option<(OwnedFd, Given)> {
    let mut given = Given::new();
    let dir = plain_files(folder, |name, f, st| {
        let parts = Parts::of(st, from);
        let name = name
            .to_str()
            .ok()
            .filter(|n| *n != GIVEN && !n.contains('\n'));
        if parts.any() && parts.give(f, to) {
            given.extend(name.map(|n| (parts, n.to_string())));
        }
    })?;
    let parts = rustix::fs::fstat(&dir).map(|st| Parts::of(&st, from));
    if let Ok(parts) = parts
        && parts.any()
        && parts.give(&dir, to)
    {
        given.push((parts, FOLDER.to_string()));
    }
    Some((dir, given))
}

pub(in crate::service) fn to_root(config: &Path, account: &str) {
    let Ok(ids) = crate::base::process::account_ids(account) else {
        return;
    };
    if ids.0 == 0 {
        return;
    }
    let Some((dir, given)) = move_parts(&drop_in_folder(config), ids, (0, 0)) else {
        return;
    };
    if given.is_empty() {
        return;
    }
    let mut all = read(&dir);
    all.retain(|(_, n)| !given.iter().any(|(_, g)| g == n));
    all.extend(given);
    write(&dir, &all);
}

pub(in crate::service) fn hand_over(
    config: &Path,
    ((from, recorded), to): ((&str, Option<(u32, u32)>), &str),
    writable: bool,
) {
    let ids = crate::base::process::account_ids;
    let Ok(new) = ids(to) else {
        return;
    };
    match recorded.or_else(|| ids(from).ok()) {
        Some(old) if old == new => {}
        Some((0, _)) => super::place::give_copied(config, to, writable),
        Some(old) => {
            move_parts(&drop_in_folder(config), old, new);
        }
        None => hand_over_unknown(config, new),
    }
}

fn hand_over_unknown(config: &Path, new: (u32, u32)) {
    let folder = drop_in_folder(config);
    let Ok(dir) = rustix::fs::open(&folder, crate::base::fs::NO_FOLLOW_FOLDER, Mode::empty())
    else {
        return;
    };
    if let Ok(st) = rustix::fs::fstat(&dir)
        && st.st_uid != 0
    {
        move_parts(&folder, (st.st_uid, st.st_gid), new);
        return;
    }
    let entries: Vec<(OwnedFd, Stat)> = super::place::copied_names(&dir)
        .unwrap_or_default()
        .iter()
        .filter_map(|n| super::place::plain_at(&dir, n))
        .filter_map(|f| {
            let st = rustix::fs::fstat(&f).ok()?;
            Some((f, st))
        })
        .collect();
    let uid = entries.iter().map(|(_, st)| st).find(|st| st.st_uid != 0);
    let gid = entries.iter().map(|(_, st)| st).find(|st| st.st_gid != 0);
    let old = (
        uid.map_or(0, |st| st.st_uid),
        uid.or(gid).map_or(0, |st| st.st_gid),
    );
    for (f, st) in &entries {
        let parts = Parts::of(st, old);
        if parts.any() {
            parts.give(f, new);
        }
    }
}

pub(in crate::service) fn give_back(config: &Path, account: &str) {
    let folder = drop_in_folder(config);
    let Ok(dir) = rustix::fs::open(&folder, crate::base::fs::NO_FOLLOW_FOLDER, Mode::empty())
    else {
        return;
    };
    let given = read(&dir);
    let Ok(ids) = crate::base::process::account_ids(account) else {
        return;
    };
    if given.is_empty() {
        return;
    }
    for (parts, name) in &given {
        if let Some((f, st)) = opened(&dir, name) {
            let still = Parts {
                owner: parts.owner && st.st_uid == 0,
                group: parts.group && st.st_gid == 0,
            };
            if still.any() {
                still.give(&f, ids);
            }
        }
    }
    let _ = rustix::fs::unlinkat(&dir, GIVEN, AtFlags::empty());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_names_each_part_given_and_reads_back_only_plain_names() {
        let given = vec![
            (
                Parts {
                    owner: true,
                    group: true,
                },
                "apps.yaml".to_string(),
            ),
            (
                Parts {
                    owner: false,
                    group: true,
                },
                "a b.yaml".to_string(),
            ),
            (
                Parts {
                    owner: true,
                    group: false,
                },
                FOLDER.to_string(),
            ),
        ];
        let text: String = given.iter().map(line).collect();
        assert_eq!(text, "ug apps.yaml\n-g a b.yaml\nu- .\n");
        assert_eq!(text.lines().filter_map(parse).collect::<Given>(), given);
        for bad in [
            "-- apps.yaml",
            "ug ../x.yaml",
            "ug ..",
            "ug ",
            "u apps.yaml",
            "ugapps",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }
}
