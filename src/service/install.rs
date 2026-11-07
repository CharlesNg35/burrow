//! Installs or reinstalls burrow as a system service, and puts the previous install back when a step fails.
//! Start at `install`; `place_files` copies the config and registers the unit.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::checks::{
    Checked, check_config, check_files_readable, check_ports_before_stop, check_ports_free,
    refuse_another_source, refuse_running_outside, refuse_unfit,
};
use super::previous::{
    Absent, Reached, StartAgain, Takeover, Undo, give_old_files, given_note, release_previous,
    start_installed, take_over,
};
use super::uninstall::{Kept, keep_admin_copy};
use super::{Backend, background, name_for, save_manifest, say, stray, system};
use crate::base::failure::Failure;
use crate::config::Role;
use crate::config::env::referenced_env;
use crate::install::{Manifest, Platform, all_manifests, load_manifest};

pub struct InstallOptions {
    pub config: PathBuf,
    pub name: Option<String>,
    pub run_as: Option<String>,
    pub start: bool,
    pub print: bool,
    pub worker: Option<PathBuf>,
    pub admin_writable: bool,
}

pub(super) fn recorded_source(existing: Option<&Manifest>, source: &Path) -> PathBuf {
    match existing {
        Some(m) if crate::base::fs::canonical_or_same(&m.config) == source => {
            m.source_config.clone()
        }
        _ => source.to_path_buf(),
    }
}

pub(super) fn installs_from(existing: &Manifest, source: &Path) -> bool {
    existing.source_config == source
        || crate::base::fs::canonical_or_same(&existing.config) == source
}

fn installed_config(
    folder: &Path,
    checked: &Checked,
    source: &Path,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<PathBuf, Failure> {
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("yaml");
    let config = folder.join(format!("{}.{ext}", checked.role.as_str()));
    if checked.role == Role::Server {
        super::drop_ins::check_leftovers(source, &config, env)?;
    }
    Ok(config)
}

fn say_created(created: bool, account: &str, out: &mut dyn Write) -> Result<(), Failure> {
    if created {
        say(
            out,
            format!("created service account {account} (no login, no home)"),
        )?;
    }
    Ok(())
}

pub fn install(opts: &InstallOptions, out: &mut dyn Write) -> Result<(), Failure> {
    let backend = system::backend()?;
    let platform = Platform::current();
    let source = readable_source(opts)?;
    let name = name_for(&source, opts.name.as_deref(), &all_manifests())?;
    let src_text = std::fs::read_to_string(&source).unwrap_or_default();
    let env = env_for(opts, &src_text);
    let checked = check_config(&source, &env)?;
    refuse_another_source(&name, &source)?;
    let account = opts
        .run_as
        .clone()
        .unwrap_or_else(|| platform.default_account(&name));
    let short = crate::install::short_name(&name).to_string();
    let folder = platform.config_dir().join(&short);
    let writable =
        checked.role == Role::Server && (opts.admin_writable || !checked.admins.is_empty());
    let config = installed_config(&folder, &checked, &source, &env)?;
    let env_file = (!env.is_empty()).then(|| platform.config_dir().join(format!("{short}.env")));
    let current = std::env::current_exe()
        .map_err(|e| Failure::new(format!("cannot locate the burrow binary: {e}")))?;
    let current = crate::base::fs::canonical(&current).unwrap_or(current);
    let binary = platform.service_binary();
    let low_ports = checked.listens.iter().any(|(_, a, _)| a.port() < 1024);
    let manifest = Manifest {
        name: name.clone(),
        role: checked.role,
        config: config.clone(),
        source_config: recorded_source(load_manifest(&name).ok().as_ref(), &source),
        env_file: env_file.clone(),
        log_file: platform.log_file(&short),
        binary: binary.clone(),
        copied_binary: true,
        account: account.clone(),
        created_account: false,
        low_ports,
        files: checked.files.iter().map(|(_, p)| p.clone()).collect(),
        version: crate::base::build::version().to_string(),
        admin_writable: writable,
        web_state: (checked.needs_state && checked.role == Role::Server)
            .then(|| crate::install::service_state_dir(&short)),
        remote_updates: checked.remote_updates,
    };
    refuse_unfit(&*backend, &checked, &manifest)?;

    if opts.print {
        return print_unit(&*backend, &name, &manifest, out);
    }

    system::require_admin()?;
    let taken = take_over(stray::named(&name).as_ref(), &config)?;
    let old_files: Vec<PathBuf> = taken
        .iter()
        .flat_map(Takeover::old_files)
        .chain(stray::leftovers(Some(&name)))
        .collect();
    let operation = system::operation(out)?;
    let previous = load_manifest(&name).ok();
    let was_running = previous.as_ref().is_some_and(|p| backend.status(p).running);
    if was_running && opts.worker.is_none() {
        drop(operation);
        return reinstall_in_background(opts, &source, &name, &env, out);
    }
    let _operation = operation;
    let mut again = StartAgain {
        backend: &*backend,
        manifest: previous.as_ref().filter(|_| was_running),
    };
    stop_when_free(&*backend, &checked, again.manifest)?;

    refuse_running_outside(&checked)?;
    check_ports_free(&*backend, &checked)?;

    let created = backend.ensure_account(&account)?;
    let mut fresh = FreshAccount {
        backend: &*backend,
        account: created.then_some(account.as_str()),
    };
    say_created(created, &account, out)?;
    let manifest = Manifest {
        created_account: burrows_account(
            created,
            &account,
            &platform.default_account(&name),
            previous.as_ref(),
            || backend.made_for_burrow(&account),
        ),
        ..manifest
    };
    check_files_readable(&*backend, &checked, &account)?;

    let absent = previous.as_ref().map(|p| Absent::before(p, &manifest));
    backend.prepare_dirs(&account)?;
    super::uninstall::say_skipped(&backend.state_folder(&manifest)?, &account, out)?;
    let kept_binary = keep_previous_binary(again.manifest, &binary, &current);
    let _leftover = Leftover(kept_binary.clone());
    if !system::same_content(&current, &binary) {
        copy_service_binary(&current, &binary, platform, &name, out)?;
    }
    let undo = previous.as_ref().zip(absent).map(|(p, absent)| Undo {
        manifest: p.clone(),
        config: system::read_plain_file(&p.config),
        drop_ins: super::drop_ins::Snapshot::take(&p.config),
        env: backend.read_env(p),
        binary: kept_binary.clone().map(|kept| (kept, binary.clone())),
        absent,
    });
    backend.config_folder(&folder, &config, &account, writable)?;
    place_files(
        &*backend,
        undo.as_ref(),
        (&source, &folder),
        (&manifest, &env, &checked.admins),
        |out| {
            super::helper::set_up(&manifest, out)?;
            if !opts.start {
                again.manifest = None;
            }
            save_manifest(&manifest)
        },
        out,
    )?;
    fresh.account = None;
    say_installed(
        &*backend,
        &manifest,
        &config,
        &old_files,
        taken.as_ref(),
        out,
    )?;
    let arg = name_arg(&name, all_manifests().len(), stray::strays().len());
    if opts.start {
        start_installed(
            &*backend,
            &manifest,
            &arg,
            &mut again,
            undo.filter(|_| was_running),
            save_manifest,
            out,
        )?;
    }
    if let Some(p) = &previous {
        release_previous(&*backend, p, &manifest, out)?;
    }
    say_next_steps(&config, &arg, out)
}

pub(super) fn place_files(
    backend: &dyn Backend,
    undo: Option<&Undo>,
    paths: (&Path, &Path),
    setup: (&Manifest, &BTreeMap<String, String>, &[String]),
    finish: impl FnOnce(&mut dyn Write) -> Result<(), Failure>,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let mut reached = Reached::Files;
    let placed = place(backend, &mut reached, paths, setup, finish, out);
    if placed.is_err()
        && let Some(undo) = undo
    {
        let _ = undo.restore_setup(backend, reached);
    }
    placed
}

fn place(
    backend: &dyn Backend,
    reached: &mut Reached,
    (source, folder): (&Path, &Path),
    (m, env, admins): (&Manifest, &BTreeMap<String, String>, &[String]),
    finish: impl FnOnce(&mut dyn Write) -> Result<(), Failure>,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let (config, account, writable) = (&m.config, m.account.as_str(), m.admin_writable);
    let copies = source != crate::base::fs::canonical_or_same(config);
    if copies {
        copy_config(backend, (source, config), (account, writable), out)?;
    }
    #[cfg(unix)]
    if !copies {
        super::uninstall::give_locked_drop_ins(config, account);
    }
    backend.secure_file(config, account)?;
    backend.config_folder(folder, config, account, writable)?;
    if writable {
        say_writable(admins, folder, out)?;
    }
    if let Some(f) = &m.env_file {
        *reached = Reached::Env;
        save_env(backend, m, env, f, out)?;
    }
    *reached = Reached::Unit;
    backend.install(m)?;
    finish(out)
}

fn readable_source(opts: &InstallOptions) -> Result<PathBuf, Failure> {
    crate::base::fs::canonical(&opts.config).map_err(|e| {
        Failure::usage(format!(
            "cannot read {}: {}",
            opts.config.display(),
            crate::base::failure::file_reason(&e)
        ))
    })
}

fn print_unit(
    backend: &dyn Backend,
    name: &str,
    manifest: &Manifest,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    say(
        out,
        format!(
            "# {} service {name}, as installed by: burrow service install",
            backend.kind()
        ),
    )?;
    say(out, backend.render(manifest))?;
    Ok(())
}

fn keep_previous_binary(
    previous: Option<&Manifest>,
    binary: &Path,
    current: &Path,
) -> Option<PathBuf> {
    (previous.is_some_and(|p| p.binary == binary)
        && binary.exists()
        && !system::same_content(current, binary))
    .then(|| binary.with_extension("reinstall"))
    .filter(|kept| system::copy_file(binary, kept, true).is_ok())
}

fn save_env(
    backend: &dyn Backend,
    manifest: &Manifest,
    env: &BTreeMap<String, String>,
    file: &Path,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    backend.write_env(manifest, env)?;
    say(
        out,
        format!(
            "saved {} for the service in {} (readable by the service only)",
            env.keys().cloned().collect::<Vec<_>>().join(", "),
            file.display()
        ),
    )
}

fn say_installed(
    backend: &dyn Backend,
    manifest: &Manifest,
    config: &Path,
    old_files: &[PathBuf],
    taken: Option<&Takeover>,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let name = &manifest.name;
    let account = &manifest.account;
    say(
        out,
        format!(
            "installed {} service {name} ({}, running as {account}, starts at boot, restarts on failure)",
            backend.kind(),
            manifest.role
        ),
    )?;
    let mut own = crate::base::logfile::all_files(&manifest.log_file);
    own.push(config.to_path_buf());
    own.extend(manifest.env_file.clone());
    let given = give_old_files(old_files, account, &own);
    if let Some(t) = taken {
        say(out, format!("note: {}", t.note(&given)))?;
    }
    if let Some(note) = given_note(name, &given, taken.and_then(|t| t.old_config.as_ref())) {
        say(out, format!("note: {note}"))?;
    }
    Ok(())
}

fn env_for(opts: &InstallOptions, src_text: &str) -> BTreeMap<String, String> {
    match &opts.worker {
        Some(log) => background::take_secrets(log),
        None => referenced_env(src_text)
            .into_iter()
            .filter_map(|k| std::env::var(&k).ok().map(|v| (k, v)))
            .collect(),
    }
}

fn stop_when_free(
    backend: &dyn Backend,
    checked: &Checked,
    running: Option<&Manifest>,
) -> Result<(), Failure> {
    if let Some(p) = running {
        check_ports_before_stop(backend, checked, p)?;
        backend.stop(p)?;
    }
    Ok(())
}

fn reinstall_in_background(
    opts: &InstallOptions,
    source: &Path,
    name: &str,
    env: &BTreeMap<String, String>,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let mut args: Vec<std::ffi::OsString> = vec![
        "service".into(),
        "install".into(),
        "--config".into(),
        source.to_path_buf().into(),
        "--name".into(),
        name.to_string().into(),
    ];
    if let Some(account) = &opts.run_as {
        args.push("--run-as".into());
        args.push(account.into());
    }
    if !opts.start {
        args.push("--no-start".into());
    }
    if opts.admin_writable {
        args.push("--admin-writable".into());
    }
    background::Run::new("install")?.start(&format!("reinstalling {name}"), args, Some(env), out)
}

fn copy_service_binary(
    current: &Path,
    binary: &Path,
    platform: Platform,
    name: &str,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    place_binary(current, binary, &platform.binary_path())?;
    say(
        out,
        format!("copied {} -> {}", current.display(), binary.display()),
    )?;
    if let Some(usual) = platform.binary_path().parent()
        && binary != platform.binary_path()
    {
        say(
            out,
            format!(
                "note: users other than root can change {}, so the service runs its own copy",
                usual.display()
            ),
        )?;
    }
    let others: Vec<String> = all_manifests()
        .into_iter()
        .filter(|m| m.name != name && m.copied_binary && m.binary == binary)
        .map(|m| m.name)
        .collect();
    if !others.is_empty() {
        say(
            out,
            format!(
                "note: {} still run the previous binary until restarted; restart them all with: {}",
                others.join(", "),
                crate::base::failure::admin("service update --force")
            ),
        )?;
    }
    Ok(())
}

pub(super) fn copy_config(
    backend: &dyn super::Backend,
    (source, config): (&Path, &Path),
    (account, writable): (&str, bool),
    out: &mut dyn Write,
) -> Result<(), Failure> {
    if writable {
        match keep_admin_copy(source, config)? {
            Kept::Version => say(
                out,
                "kept the installed config as a history version before replacing it; revert to it from burrow admin Activity if needed",
            )?,
            Kept::NotPlain => say(
                out,
                format!(
                    "note: {} is not a plain file, so it is replaced without keeping it",
                    config.display()
                ),
            )?,
            Kept::Nothing => {}
        }
    }
    system::copy_file(source, config, false)?;
    say(
        out,
        format!("copied {} -> {}", source.display(), config.display()),
    )?;
    super::drop_ins::copy(backend, (source, config), account, writable, out)
}

fn say_writable(admins: &[String], folder: &Path, out: &mut dyn Write) -> Result<(), Failure> {
    let why = match admins {
        [] => "--admin-writable was given".to_string(),
        [one] => format!("visitor {one} has admin access"),
        many => format!("visitors {} have admin access", many.join(", ")),
    };
    say(
        out,
        format!(
            "the service may change its config in {} because {why} (burrow admin)",
            folder.display()
        ),
    )
}

fn say_next_steps(config: &Path, arg: &str, out: &mut dyn Write) -> Result<(), Failure> {
    say(
        out,
        format!(
            "\nthe service uses {}; change it with {} and it applies the change by itself",
            config.display(),
            crate::base::failure::admin(&format!("edit{arg}"))
        ),
    )?;
    say(
        out,
        format!(
            "  status: {}    logs: {}",
            crate::base::failure::cmd(&format!("service status{arg}")),
            crate::base::failure::admin(&format!("service logs{arg} -f"))
        ),
    )?;
    Ok(())
}

pub(super) fn name_arg(name: &str, managed: usize, strays: usize) -> String {
    if managed + strays <= 1 {
        String::new()
    } else {
        format!(" {name}")
    }
}

pub(super) fn burrows_account(
    created: bool,
    account: &str,
    default: &str,
    previous: Option<&Manifest>,
    made_for_burrow: impl FnOnce() -> bool,
) -> bool {
    created
        || previous.is_some_and(|p| p.created_account && p.account == account)
        || (account == default && made_for_burrow())
}

struct Leftover(Option<PathBuf>);

impl Drop for Leftover {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

struct FreshAccount<'a> {
    backend: &'a dyn Backend,
    account: Option<&'a str>,
}

impl Drop for FreshAccount<'_> {
    fn drop(&mut self) {
        if let Some(account) = self.account.take() {
            let _ = self.backend.remove_account(account);
        }
    }
}

pub(super) fn place_binary(current: &Path, binary: &Path, usual: &Path) -> Result<(), Failure> {
    #[cfg(unix)]
    if let Some(dir) = binary.parent()
        && (binary != usual || !dir.exists())
    {
        system::make_dir(dir, 0o755, None)?;
    }
    #[cfg(not(unix))]
    let _ = usual;
    system::copy_file(current, binary, true)
}

#[cfg(test)]
mod tests;
