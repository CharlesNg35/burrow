use std::path::Path;

use super::{COPIED, Put, file_part, is_link_or_file, listed, place};
use crate::base::failure::Failure;
use crate::config::docs::{self, drop_in_folder};
use crate::service::{Backend, system};

#[derive(Debug)]
struct Kept {
    bytes: Vec<u8>,
    put: Put,
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
                let kept = system::read_plain_file(&path).map(|bytes| Kept { bytes, put });
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
                now.contains(name)
                    && kept.as_ref().is_none_or(|k| {
                        system::read_plain_file(&to.join(name)).as_ref() == Some(&k.bytes)
                    })
            })
    }

    pub(super) fn restore(&self, folder: &place::Folder, config: &Path) -> Result<(), Failure> {
        let mut steps = Vec::new();
        for name in present(config) {
            if !self.files.iter().any(|(n, _)| *n == name) {
                steps.push(folder.remove(&name));
            }
        }
        for (name, kept) in &self.files {
            if let Some(k) = kept {
                steps.push(folder.put(name, &k.bytes, k.put));
            }
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
