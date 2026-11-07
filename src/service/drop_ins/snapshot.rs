use std::path::Path;

use super::{COPIED, Put, file_part, is_link_or_file, listed, place};
use crate::base::failure::Failure;
use crate::config::docs::{self, drop_in_folder};
use crate::service::{Backend, system};

#[cfg(unix)]
type Stat = (u32, u32, rustix::fs::Mode);
#[cfg(not(unix))]
type Stat = ();

#[derive(Debug)]
struct Kept {
    bytes: Vec<u8>,
    put: Put,
    stat: Option<Stat>,
}

fn stat_of(path: &Path) -> Option<Stat> {
    #[cfg(unix)]
    {
        rustix::fs::lstat(path).ok().map(|st| {
            let mode = rustix::fs::Mode::from_raw_mode(st.st_mode);
            (st.st_uid, st.st_gid, mode)
        })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

impl Kept {
    fn changed(&self, path: &Path) -> bool {
        system::read_plain_file(path).as_ref() != Some(&self.bytes) || stat_of(path) != self.stat
    }
}

#[derive(Debug, Default)]
pub(in crate::service) struct Snapshot {
    files: Vec<(String, Option<Kept>)>,
}

fn present(config: &Path) -> Vec<String> {
    let to = drop_in_folder(config);
    if is_link_or_file(&to) {
        return Vec::new();
    }
    let mut names: Vec<String> = listed(config)
        .unwrap_or_default()
        .iter()
        .map(|n| file_part(n).to_string())
        .collect();
    if std::fs::symlink_metadata(to.join(COPIED)).is_ok() {
        names.push(COPIED.to_string());
    }
    names
}

impl Snapshot {
    pub(in crate::service) fn take(config: &Path) -> Snapshot {
        let to = drop_in_folder(config);
        let files = present(config)
            .into_iter()
            .map(|name| {
                let path = to.join(&name);
                let put = match docs::read_only(&path) {
                    _ if name == COPIED => Put::Note,
                    Some(_) => Put::Locked,
                    None => Put::Config,
                };
                let kept = system::read_plain_file(&path).map(|bytes| Kept {
                    bytes,
                    put,
                    stat: stat_of(&path),
                });
                (name, kept)
            })
            .collect();
        Snapshot { files }
    }

    fn unchanged(&self, config: &Path) -> bool {
        let now = present(config);
        let to = drop_in_folder(config);
        now.len() == self.files.len()
            && self.files.iter().all(|(name, kept)| {
                now.contains(name) && kept.as_ref().is_none_or(|k| !k.changed(&to.join(name)))
            })
    }

    pub(super) fn restore(&self, folder: &place::Folder, config: &Path) -> Result<(), Failure> {
        let mut steps = Vec::new();
        for name in present(config) {
            if !self.files.iter().any(|(n, _)| *n == name) {
                steps.push(folder.remove(&name));
            }
        }
        let to = drop_in_folder(config);
        for (name, k) in self
            .files
            .iter()
            .filter_map(|(n, k)| Some((n, k.as_ref()?)))
        {
            if !k.changed(&to.join(name)) {
                continue;
            }
            #[cfg(unix)]
            if let Some(stat) = k.stat {
                steps.push(folder.put_exact(name, &k.bytes, stat));
                continue;
            }
            steps.push(folder.put(name, &k.bytes, k.put));
        }
        steps.into_iter().find_map(Result::err).map_or(Ok(()), Err)
    }

    pub(in crate::service) fn put_back(
        &self,
        backend: &dyn Backend,
        config: &Path,
        account: &str,
        writable: bool,
    ) -> Result<(), Failure> {
        if self.unchanged(config) {
            return Ok(());
        }
        let to = drop_in_folder(config);
        let folder = place::Folder::open(backend, (config, &to), account, writable)?;
        self.restore(&folder, config)
    }
}
