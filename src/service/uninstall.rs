use std::io::Write;
use std::path::{Path, PathBuf};

use super::previous::give_old_files;
use super::{Backend, p_log_dir, resolve, save_manifest, say, stray, system};
use crate::base::failure::Failure;
use crate::install::{Manifest, Platform, all_manifests, manifest_path};

pub(super) fn admin_history(config: &Path) -> PathBuf {
    crate::config::save::history_dir_of(config)
}

#[cfg(unix)]
pub(super) fn give_history(config: &Path, uid: u32, gid: u32) {
    use rustix::fs::{FileType, Mode, OFlags};
    let folder = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    if let Ok(history) = rustix::fs::open(admin_history(config), folder, Mode::empty()) {
        let file = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        for entry in rustix::fs::Dir::read_from(&history)
            .into_iter()
            .flatten()
            .flatten()
        {
            let Ok(f) = rustix::fs::openat(&history, entry.file_name(), file, Mode::empty()) else {
                continue;
            };
            if rustix::fs::fstat(&f).is_ok_and(|st| {
                FileType::from_raw_mode(st.st_mode) == FileType::RegularFile && st.st_nlink == 1
            }) {
                let _ = std::os::unix::fs::fchown(&f, Some(uid), Some(gid));
            }
        }
        let _ = std::os::unix::fs::fchown(&history, Some(uid), Some(gid));
    }
}

pub(super) fn say_skipped(
    linked: &[PathBuf],
    owner: &str,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    for p in linked {
        say(
            out,
            format!(
                "warning: {} has more than one hard link, so it was not given to {owner}; burrow never makes hard links, so check who made it",
                p.display()
            ),
        )?;
    }
    Ok(())
}

pub(super) fn is_source(config: &Path, sources: &[&Path]) -> bool {
    let config = crate::base::fs::canonical_or_same(config);
    sources
        .iter()
        .any(|s| crate::base::fs::canonical_or_same(s) == config)
}

pub(super) fn delete_hint(files: &[PathBuf], folder: Option<&Path>) -> String {
    let quote = |p: &Path| format!("\"{}\"", p.display());
    let mut list: Vec<String> = folder.map(quote).into_iter().collect();
    list.extend(
        files
            .iter()
            .filter(|f| !folder.is_some_and(|d| f.starts_with(d)))
            .map(|f| quote(f)),
    );
    match (cfg!(windows), folder.is_some()) {
        (true, false) => format!("Remove-Item {}", list.join(", ")),
        (true, true) => format!("Remove-Item -Recurse {}", list.join(", ")),
        (false, false) => format!("sudo rm {}", list.join(" ")),
        (false, true) => format!("sudo rm -r {}", list.join(" ")),
    }
}

pub fn uninstall(name: Option<&str>, purge: bool, out: &mut dyn Write) -> Result<(), Failure> {
    let backend = system::backend()?;
    system::require_admin()?;
    resolve(name)?;
    let operation = system::operation(out)?;
    let m = resolve(name)?;
    let _ = backend.stop(&m);
    backend.uninstall(&m)?;
    let _ = std::fs::remove_file(manifest_path(&m.name));
    say(out, format!("removed service {}", m.name))?;
    let others = all_manifests();
    let strays = stray::strays();
    let mut logs = crate::base::logfile::all_files(&m.log_file);
    logs.extend(access_logs(&m));
    let mut kept = Vec::new();
    let mut kept_folder = None;
    let state = m.web_state.clone().filter(|d| real_dir(d));
    if purge {
        purge_files(&m, &logs, out)?;
        if let Some(d) = &state
            && std::fs::remove_dir_all(d).is_ok()
        {
            say(
                out,
                format!(
                    "removed {}, the web state: certificates, their keys, the local CA's key and uploads",
                    d.display()
                ),
            )?;
        }
    } else {
        kept.extend(
            [Some(&m.config), m.env_file.as_ref()]
                .into_iter()
                .flatten()
                .chain(logs.iter())
                .filter(|f| std::fs::symlink_metadata(f).is_ok_and(|m| m.file_type().is_file()))
                .cloned(),
        );
        let _ = std::fs::remove_dir(m.folder());
        kept_folder = Some(m.folder().to_path_buf()).filter(|d| real_dir(d));
    }
    let last_binary_user = m.copied_binary && !others.iter().any(|o| o.binary == m.binary);
    if let Some(note) = stray::keeps(&strays, m.binary.display()).filter(|_| last_binary_user) {
        say(out, note)?;
    } else if last_binary_user {
        remove_binary(&m, out)?;
    }
    let heir = others
        .iter()
        .find(|o| o.account == m.account && !o.created_account);
    if m.created_account
        && let Some(heir) = heir
        && !others
            .iter()
            .any(|o| o.account == m.account && o.created_account)
    {
        let mut heir = heir.clone();
        heir.created_account = true;
        save_manifest(&heir)?;
    }
    #[cfg(unix)]
    if let Some(d) = state.as_deref().filter(|_| !purge) {
        say_skipped(&super::hand_over::own_tree(d, 0, 0), "root", out)?;
    }
    let mut account_left = None;
    let last_account_user = m.created_account && !others.iter().any(|o| o.account == m.account);
    if let Some(note) = stray::keeps(&strays, format!("service account {}", m.account))
        .filter(|_| last_account_user)
    {
        say(out, note)?;
    } else if last_account_user {
        #[cfg(unix)]
        give_kept_to_root(&m, &kept, kept_folder.as_deref());
        account_left = remove_service_account(&*backend, &m, out)?;
    }
    if !kept.is_empty() || kept_folder.is_some() {
        say(
            out,
            format!(
                "kept these files for a later reinstall; delete them with:\n  {}",
                delete_hint(&kept, kept_folder.as_deref())
            ),
        )?;
    }
    if let Some(d) = state.as_deref().filter(|_| !purge) {
        say(
            out,
            format!(
                "kept {}, the web state: certificates, their keys, the local CA's key and uploads, for a later reinstall; --purge deletes it, and so does:\n  {}",
                d.display(),
                delete_hint(&[], Some(d))
            ),
        )?;
    }
    if others.is_empty() {
        remove_burrow_folders(operation, purge);
    }
    match account_left {
        Some(f) => Err(account_not_removed(&m.account, f)),
        None => Ok(()),
    }
}

fn real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

pub(super) fn access_logs(m: &Manifest) -> Vec<PathBuf> {
    let env = crate::config::env::ProcessEnv;
    let Ok(outcome) = crate::config::load_file(&m.config, Some(crate::config::Role::Server), &env)
    else {
        return Vec::new();
    };
    let Some(crate::config::Loaded::Server(_, cfg)) = outcome.loaded else {
        return Vec::new();
    };
    super::web_files::access_log(&cfg)
        .filter(|l| super::web_files::log_folder_refusal(l, &m.log_file, cfg!(windows)).is_none())
        .map(|l| crate::base::logfile::all_files(&l))
        .unwrap_or_default()
}

fn purge_files(m: &Manifest, logs: &[PathBuf], out: &mut dyn Write) -> Result<(), Failure> {
    if let Some(f) = &m.env_file
        && std::fs::remove_file(f).is_ok()
    {
        say(out, format!("removed {}", f.display()))?;
    }
    if real_dir(m.folder()) && std::fs::remove_dir_all(m.folder()).is_ok() {
        say(
            out,
            format!("removed {} and the files in it", m.folder().display()),
        )?;
    } else if std::fs::remove_file(&m.config).is_ok() {
        say(out, format!("removed {}", m.config.display()))?;
    }
    if logs
        .iter()
        .filter(|f| std::fs::remove_file(f).is_ok())
        .count()
        > 0
    {
        say(out, format!("removed the logs in {}", m.log_file.display()))?;
    }
    if let Some(dir) = m.log_file.parent().filter(|d| *d != p_log_dir()) {
        let _ = std::fs::remove_dir(dir);
    }
    Ok(())
}

fn remove_binary(m: &Manifest, out: &mut dyn Write) -> Result<(), Failure> {
    match std::fs::remove_file(&m.binary) {
        Ok(()) => {
            let _ = std::fs::remove_file(m.binary.with_extension("previous"));
            let _ = std::fs::remove_file(m.binary.with_extension("old"));
            let _ = std::fs::remove_file(m.binary.with_extension("reinstall"));
            let _ = std::fs::remove_file(m.binary.with_extension("burrow-new"));
            let own_folder = cfg!(windows) || m.binary != Platform::current().binary_path();
            if own_folder && let Some(dir) = m.binary.parent() {
                let _ = std::fs::remove_dir(dir);
            }
            say(out, format!("removed {}", m.binary.display()))?;
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            let later = match m.binary.parent().filter(|_| cfg!(windows)) {
                Some(dir) => format!(
                    "Remove-Item -Recurse {}",
                    crate::base::failure::path_arg(dir)
                ),
                None => format!("sudo rm {}", crate::base::failure::path_arg(&m.binary)),
            };
            say(
                out,
                format!(
                    "note: cannot remove {} ({}); delete it after this command ends: {later}",
                    m.binary.display(),
                    crate::base::failure::file_reason(&e)
                ),
            )?;
        }
        Err(_) => {}
    }
    Ok(())
}

#[cfg(unix)]
fn give_kept_to_root(m: &Manifest, kept: &[PathBuf], kept_folder: Option<&Path>) {
    let dir = m
        .log_file
        .parent()
        .filter(|d| !kept.is_empty() && *d != p_log_dir());
    for f in kept.iter().map(PathBuf::as_path) {
        let _ = system::give_plain_file(f, 0, 0);
    }
    if let Some(d) = dir {
        let _ = std::os::unix::fs::lchown(d, Some(0), Some(0));
    }
    if let Some(d) = kept_folder {
        give_history(&m.config, 0, 0);
        let _ = std::os::unix::fs::lchown(d, Some(0), Some(0));
    }
}

fn remove_service_account(
    backend: &dyn Backend,
    m: &Manifest,
    out: &mut dyn Write,
) -> Result<Option<Failure>, Failure> {
    let given = give_old_files(&stray::leftovers(None), &m.account, &[]);
    if !given.is_empty() {
        say(
            out,
            format!(
                "gave {} to root before removing service account {}, since an older install left {} owned by it",
                given
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                m.account,
                if given.len() == 1 { "it" } else { "them" }
            ),
        )?;
    }
    match backend.remove_account(&m.account) {
        Ok(()) => say(out, format!("removed service account {}", m.account))?,
        Err(f) => return Ok(Some(f)),
    }
    Ok(None)
}

fn remove_burrow_folders(operation: system::Operation, purge: bool) {
    let p = Platform::current();
    operation.remove();
    let _ = std::fs::remove_dir_all(p.jobs_dir());
    let mut dirs = vec![p.state_dir()];
    dirs.extend(p.state_dir().parent().map(Path::to_path_buf));
    if purge {
        dirs.extend([p.log_dir(), p.config_dir()]);
    }
    let locks = crate::instance::service_dir().unwrap_or_else(crate::instance::default_dir);
    crate::instance::remove_unused(&locks);
    for dir in dirs {
        let _ = std::fs::remove_dir(dir);
    }
}

fn account_not_removed(account: &str, f: Failure) -> Failure {
    let finish = match Platform::current() {
        Platform::MacOs => format!(
            "macOS lets only a local session or an app with full disk access delete accounts; finish in Terminal on the Mac with: sudo dscl . -delete /Users/{account}  (or allow full disk access for remote users under System Settings > General > Sharing > Remote Login, then run it over SSH)"
        ),
        _ => format!("finish with: sudo userdel {account}"),
    };
    Failure::new(format!(
        "everything else is removed, but the service account {account} is not: {}",
        f.message
    ))
    .hint(finish)
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Kept {
    Nothing,
    Version,
    NotPlain,
}

pub(super) fn keep_admin_copy(source: &Path, config: &Path) -> Result<Kept, Failure> {
    use crate::admin::core::ports::Store;
    if std::fs::symlink_metadata(config).is_err() {
        return Ok(Kept::Nothing);
    }
    let Some(installed) = system::read_plain_file(config) else {
        return Ok(Kept::NotPlain);
    };
    let (Ok(src), Ok(installed)) = (std::fs::read(source), String::from_utf8(installed)) else {
        return Ok(Kept::Nothing);
    };
    if src == installed.as_bytes() {
        return Ok(Kept::Nothing);
    }
    let store = crate::admin::adapters::FileStore::new(config);
    store.keep(&crate::admin::core::text::rev(&installed), &installed);
    match store.history_problem() {
        None => Ok(Kept::Version),
        Some(why) => Err(Failure::new(format!(
            "{} differs from the source and cannot be kept before the reinstall replaces it: {why}",
            config.display()
        ))
        .hint(format!(
            "copy {} somewhere safe, or use it as the source: {}",
            config.display(),
            crate::base::failure::admin(&format!(
                "service install -c {} --admin-writable",
                crate::base::failure::path_arg(config)
            ))
        ))),
    }
}

pub(super) fn differs(source: &Path, installed: &Path) -> bool {
    match (std::fs::read(source), std::fs::read(installed)) {
        (Ok(a), Ok(b)) => a != b,
        _ => false,
    }
}

pub(super) fn missing(p: &Path) -> &'static str {
    if p.exists() { "" } else { "  (missing!)" }
}
