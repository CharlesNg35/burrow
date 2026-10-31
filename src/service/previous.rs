use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use super::give_history;
use super::{
    Backend, admin_history, delete_hint, install_args, is_source, save_manifest, say, stray, system,
};
use crate::base::failure::Failure;
use crate::install::{Manifest, all_manifests};

#[derive(Debug)]
pub(super) struct Takeover {
    label: String,
    pub(super) old_config: Option<PathBuf>,
}

impl Takeover {
    pub(super) fn old_files(&self) -> Vec<PathBuf> {
        self.old_config.iter().cloned().collect()
    }

    pub(super) fn note(&self, given: &[PathBuf]) -> String {
        let replaced = format!("replaced {}, which burrow had no record of", self.label);
        match &self.old_config {
            Some(old) if given.contains(old) => format!(
                "{replaced}; its old config was left as it was, now owned by root: {}",
                old.display()
            ),
            Some(old) => format!(
                "{replaced}; its old config was left as it was: {}",
                old.display()
            ),
            None => replaced,
        }
    }
}

pub(super) fn take_over(
    stray: Option<&stray::Stray>,
    config: &Path,
) -> Result<Option<Takeover>, Failure> {
    let Some(s) = stray else {
        return Ok(None);
    };
    if s.running {
        return Err(s.failure());
    }
    Ok(Some(Takeover {
        label: s.label.clone(),
        old_config: s.config.clone().filter(|c| c != config),
    }))
}

pub(super) fn given_note(name: &str, given: &[PathBuf], shown: Option<&PathBuf>) -> Option<String> {
    let rest: Vec<String> = given
        .iter()
        .filter(|p| Some(*p) != shown)
        .map(|p| p.display().to_string())
        .collect();
    (!rest.is_empty()).then(|| {
        format!(
            "gave {}, left by an older install of {name}, to root, since the service does not use {}",
            rest.join(", "),
            if rest.len() == 1 { "it" } else { "them" }
        )
    })
}

pub(super) fn give_old_files(files: &[PathBuf], account: &str, keep: &[PathBuf]) -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        let Ok((uid, _)) = crate::base::process::account_ids(account) else {
            return Vec::new();
        };
        let mut given: Vec<PathBuf> = Vec::new();
        for f in files {
            if uid != 0
                && !keep.contains(f)
                && !given.contains(f)
                && system::give_back(f, uid, 0, 0).unwrap_or(false)
            {
                given.push(f.clone());
            }
        }
        given
    }
    #[cfg(not(unix))]
    {
        let _ = (files, account, keep);
        Vec::new()
    }
}

pub(super) fn start_installed(
    backend: &dyn Backend,
    manifest: &Manifest,
    arg: &str,
    again: &mut StartAgain,
    undo: Option<Undo>,
    save: impl Fn(&Manifest) -> Result<(), Failure>,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    if already_running(backend, manifest, arg, out)? {
        return Ok(());
    }
    let started = backend.start(manifest);
    again.manifest = None;
    if started.is_ok() {
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
    let st = backend.status(manifest);
    if let Some(undo) = undo.filter(|_| started.is_err() || !st.running) {
        let why = match started {
            Err(f) => f.message,
            Ok(()) => st.detail.clone(),
        };
        return Err(undo.put_back(backend, manifest, &why, save, out));
    }
    started?;
    if st.running {
        say(
            out,
            format!(
                "started {}{}",
                manifest.name,
                st.pid.map(|p| format!(" (pid {p})")).unwrap_or_default()
            ),
        )?;
    } else {
        say(
            out,
            format!("warning: {} is not running: {}", manifest.name, st.detail),
        )?;
        match backend
            .journal_hint(manifest)
            .filter(|_| !manifest.log_file.exists())
        {
            Some(journal) => say(
                out,
                format!(
                    "  it wrote no log yet, so it may never have started; see the service manager's reason with: {journal}"
                ),
            )?,
            None => say(
                out,
                format!(
                    "  see why with: {}",
                    crate::base::failure::admin(&format!("service logs{arg}"))
                ),
            )?,
        }
    }
    Ok(())
}

fn already_running(
    backend: &dyn Backend,
    m: &Manifest,
    arg: &str,
    out: &mut dyn Write,
) -> Result<bool, Failure> {
    let st = backend.status(m);
    if !st.running {
        return Ok(false);
    }
    say(
        out,
        format!(
            "warning: {} was already running and was not restarted, so it still runs its previous setup{}; restart it with: {}",
            m.name,
            st.pid.map(|p| format!(" (pid {p})")).unwrap_or_default(),
            crate::base::failure::admin(&format!("service restart{arg}"))
        ),
    )?;
    Ok(true)
}

pub(super) struct StartAgain<'a> {
    pub(super) backend: &'a dyn Backend,
    pub(super) manifest: Option<&'a Manifest>,
}

impl Drop for StartAgain<'_> {
    fn drop(&mut self) {
        if let Some(m) = self.manifest.take() {
            let _ = self.backend.start(m);
        }
    }
}

pub(super) struct Undo {
    pub(super) manifest: Manifest,
    pub(super) config: Option<Vec<u8>>,
    pub(super) env: BTreeMap<String, String>,
    pub(super) binary: Option<(PathBuf, PathBuf)>,
}

impl Undo {
    pub(super) fn put_back(
        self,
        backend: &dyn Backend,
        new: &Manifest,
        why: &str,
        save: impl Fn(&Manifest) -> Result<(), Failure>,
        out: &mut dyn Write,
    ) -> Failure {
        let _ = backend.stop(new);
        let p = &self.manifest;
        let mut steps: Vec<Result<(), Failure>> = Vec::new();
        if let Some(bytes) = &self.config {
            steps.push(system::write_file_mode(&p.config, bytes, 0o600));
            steps.push(backend.secure_file(&p.config, &p.account));
        }
        steps.push(backend.config_folder(p.folder(), &p.config, &p.account, p.admin_writable));
        if p.env_file.is_some() {
            steps.push(backend.write_env(p, &self.env));
        }
        if let Some((kept, binary)) = &self.binary {
            steps.push(system::copy_file(kept, binary, true));
            let _ = std::fs::remove_file(kept);
        }
        steps.push(backend.install(p));
        steps.push(save(p));
        steps.push(backend.start(p));
        if let Some(env) = &new.env_file
            && p.env_file.as_ref() != Some(env)
        {
            let _ = std::fs::remove_file(env);
        }
        if new.created_account && new.account != p.account {
            let _ = backend.remove_account(&new.account);
        }
        let mut failed = steps.into_iter().find_map(Result::err);
        if failed.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let st = backend.status(p);
            if !st.running {
                failed = Some(Failure::new(format!(
                    "the previous one does not run either: {}",
                    st.detail
                )));
            }
        }
        let _ = say(
            out,
            format!("error: the reinstalled {} did not run: {why}", new.name),
        );
        match failed {
            None => Failure::new(format!(
                "the reinstalled {} did not run, so its previous setup was put back and it runs again",
                new.name
            ))
            .hint(format!(
                "see why the new one failed with: {}",
                crate::base::failure::admin(&format!("service logs {} -n 50", new.name))
            )),
            Some(f) => Failure::new(format!(
                "the reinstalled {} did not run, and putting back its previous setup failed too: {}",
                new.name, f.message
            ))
            .hint(format!(
                "install it again from a config that works: {}",
                crate::base::failure::admin(&install_args(p, &p.config))
            )),
        }
    }
}

pub(super) fn release_previous(
    backend: &dyn Backend,
    old: &Manifest,
    new: &Manifest,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let others: Vec<Manifest> = all_manifests()
        .into_iter()
        .filter(|m| m.name != new.name)
        .collect();
    if let Some(env) = &old.env_file
        && new.env_file.as_ref() != Some(env)
        && std::fs::remove_file(env).is_ok()
    {
        say(
            out,
            format!(
                "removed {}, which the service no longer uses",
                env.display()
            ),
        )?;
    }
    if old.config != new.config
        && std::fs::symlink_metadata(&old.config).is_ok_and(|m| m.file_type().is_file())
    {
        let history = admin_history(&old.config);
        let history = std::fs::symlink_metadata(&history)
            .is_ok_and(|m| m.is_dir())
            .then_some(history);
        #[cfg(unix)]
        {
            let _ = system::give_plain_file(&old.config, 0, 0);
            give_history(&old.config, 0, 0);
        }
        if !is_source(&old.config, &[&old.source_config, &new.source_config]) {
            say(
                out,
                format!(
                    "note: {} holds the config the service used before, secrets included; the service no longer uses it, so delete it with: {}",
                    old.config.display(),
                    delete_hint(std::slice::from_ref(&old.config), history.as_deref())
                ),
            )?;
        }
    }
    if old.copied_binary
        && (old.binary != new.binary || !new.copied_binary)
        && !others
            .iter()
            .any(|o| o.copied_binary && o.binary == old.binary)
        && std::fs::remove_file(&old.binary).is_ok()
    {
        let _ = std::fs::remove_file(old.binary.with_extension("previous"));
        say(
            out,
            format!(
                "removed {}, which the service no longer uses",
                old.binary.display()
            ),
        )?;
    }
    if old.created_account && old.account != new.account {
        match others.iter().find(|o| o.account == old.account) {
            Some(heir) if !heir.created_account => {
                save_manifest(&Manifest {
                    created_account: true,
                    ..heir.clone()
                })?;
            }
            Some(_) => {}
            None => match backend.remove_account(&old.account) {
                Ok(()) => say(
                    out,
                    format!(
                        "removed service account {}, which nothing uses any more",
                        old.account
                    ),
                )?,
                Err(f) => say(
                    out,
                    format!(
                        "note: could not remove the old account {}: {}",
                        old.account, f.message
                    ),
                )?,
            },
        }
    }
    Ok(())
}
