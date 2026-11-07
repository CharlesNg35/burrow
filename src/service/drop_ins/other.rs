use std::path::{Path, PathBuf};

use crate::base::failure::Failure;
use crate::service::{Backend, system};

pub(super) struct Folder<'a> {
    backend: &'a dyn Backend,
    to: PathBuf,
    account: String,
}

fn writable_again(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path)
        && meta.permissions().readonly()
    {
        let mut perms = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(path, perms);
    }
}

impl<'a> Folder<'a> {
    pub(super) fn open(
        backend: &'a dyn Backend,
        (config, to): (&Path, &Path),
        account: &str,
        writable: bool,
    ) -> Result<Folder<'a>, Failure> {
        backend.config_folder(to, config, account, writable)?;
        super::refuse_link(to)?;
        Ok(Folder {
            backend,
            to: to.to_path_buf(),
            account: account.to_string(),
        })
    }

    pub(super) fn put(&self, file: &str, text: &[u8], put: super::Put) -> Result<(), Failure> {
        let dst = self.to.join(file);
        writable_again(&dst);
        system::write_file_mode(&dst, text, 0o600)?;
        self.backend.secure_file(&dst, &self.account)?;
        if put == super::Put::Locked {
            let mut perms = std::fs::metadata(&dst)
                .map_err(|e| Failure::new(format!("cannot read {}: {e}", dst.display())))?
                .permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(&dst, perms).map_err(|e| {
                Failure::new(format!("cannot mark {} read-only: {e}", dst.display()))
            })?;
        }
        Ok(())
    }

    pub(super) fn remove(&self, file: &str) -> Result<(), Failure> {
        let gone = self.to.join(file);
        writable_again(&gone);
        std::fs::remove_file(&gone)
            .map_err(|e| Failure::new(format!("cannot remove {}: {e}", gone.display())))
    }
}
